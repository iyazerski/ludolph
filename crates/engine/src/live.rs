//! A working chat's live tail, built from pi's events.
//!
//! pi's events carry no entry ids, and pi sends `message_end` before it
//! appends the entry to the session file. So the transcript shows the tail's
//! messages until the file has them, matched by role and timestamp
//! (`docs/design.md`, "Live view and the file").

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::pi::Event;

/// What pi has done since the run started, and whether it's still going.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Live {
    /// Bumped by every change a viewer would see.
    revision: u64,
    working: bool,
    /// Runs that settled, so a caller can tell whether one ended since it
    /// looked.
    #[serde(skip)]
    settled: u64,
    /// Messages pi finished in this run, from `message_end`.
    messages: Vec<Value>,
    /// The assistant message pi is streaming, rebuilt from its deltas.
    streaming: Option<Value>,
    /// Tools that are running, by tool call id: their name, arguments and
    /// latest partial result.
    tools: BTreeMap<String, Value>,
    steering: Vec<String>,
    follow_up: Vec<String>,
    /// The automatic retry pi is waiting on, if any.
    retry: Option<Retry>,
    compacting: bool,
    /// Why the run failed, when pi gave up retrying.
    failure: Option<String>,
}

/// An automatic retry after a provider error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Retry {
    pub attempt: u32,
    pub max_attempts: u32,
    pub error: Option<String>,
}

impl Live {
    /// An empty tail, for a chat that isn't working.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one event in, and return whether anything a viewer sees changed.
    pub fn apply(&mut self, event: &Event) -> bool {
        let changed = self.fold(event);
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// Drop the finished messages, once the session file has them.
    pub fn clear_messages(&mut self) {
        if !self.messages.is_empty() {
            self.messages.clear();
            self.revision += 1;
        }
    }

    /// The counter that every visible change bumps.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether a run is going: between `agent_start` and `agent_settled`.
    pub fn working(&self) -> bool {
        self.working
    }

    /// How many runs have settled.
    pub fn settled(&self) -> u64 {
        self.settled
    }

    /// Count a run as going from when pi answered that it started, ahead of
    /// its `agent_start`, which comes through the event queue. Nothing
    /// changes if a run settled since `settled` was read: that was this
    /// one, already over. Returns whether it changed anything.
    pub fn started(&mut self, settled: u64) -> bool {
        if self.working || self.settled != settled {
            return false;
        }
        self.working = true;
        self.failure = None;
        self.revision += 1;
        true
    }

    /// The messages pi finished in this run.
    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    /// The assistant message pi is streaming.
    pub fn streaming(&self) -> Option<&Value> {
        self.streaming.as_ref()
    }

    /// Tools that are running, by tool call id.
    pub fn tools(&self) -> &BTreeMap<String, Value> {
        &self.tools
    }

    /// Why the last run failed, when pi gave up retrying.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn fold(&mut self, event: &Event) -> bool {
        match event {
            Event::AgentStart => {
                self.working = true;
                self.failure = None;
            }
            Event::AgentSettled { .. } => {
                self.working = false;
                self.settled += 1;
                self.streaming = None;
                self.tools.clear();
                self.retry = None;
                self.compacting = false;
            }
            Event::MessageStart { message } => {
                if message["role"] != "assistant" {
                    return false;
                }
                let mut message = message.clone();
                if !message["content"].is_array() {
                    message["content"] = json!([]);
                }
                self.streaming = Some(message);
            }
            Event::MessageUpdate {
                assistant_message_event,
            } => {
                let Some(streaming) = &mut self.streaming else {
                    return false;
                };
                return update_block(streaming, assistant_message_event);
            }
            Event::MessageEnd { message } => {
                if message["role"] == "assistant" {
                    self.streaming = None;
                    // A retry clears it; one that gives up sets its own.
                    match message["stopReason"].as_str() {
                        Some("error") => {
                            let error = message["errorMessage"].as_str();
                            self.failure = Some(error.unwrap_or("the request failed").into());
                        }
                        Some("length") => {
                            self.failure = Some("The reply hit the model's length limit.".into());
                        }
                        _ => {}
                    }
                }
                self.messages.push(slim(message));
            }
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                self.tools.insert(
                    tool_call_id.clone(),
                    json!({"toolName": tool_name, "args": args, "partialResult": null}),
                );
            }
            Event::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
            } => {
                let Some(tool) = self.tools.get_mut(tool_call_id) else {
                    return false;
                };
                tool["partialResult"] = partial_result.clone();
            }
            Event::ToolExecutionEnd { tool_call_id, .. } => {
                return self.tools.remove(tool_call_id).is_some();
            }
            Event::QueueUpdate {
                steering,
                follow_up,
            } => {
                self.steering.clone_from(steering);
                self.follow_up.clone_from(follow_up);
            }
            Event::AutoRetryStart {
                attempt,
                max_attempts,
                error_message,
            } => {
                self.failure = None;
                self.retry = Some(Retry {
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    error: error_message.clone(),
                });
            }
            Event::AutoRetryEnd {
                success,
                final_error,
            } => {
                self.retry = None;
                if !success {
                    self.failure = Some(
                        final_error
                            .clone()
                            .unwrap_or_else(|| "the request failed".into()),
                    );
                }
            }
            Event::CompactionStart => self.compacting = true,
            Event::CompactionEnd => self.compacting = false,
            _ => return false,
        }
        true
    }
}

/// Apply one `message_update` to the streaming message's content blocks
/// (pi's `docs/json.md`, "Reconstruct streaming messages"). Deltas build a
/// block up, and each block's end replaces it with its final content.
fn update_block(message: &mut Value, update: &Value) -> bool {
    let Some(index) = update["contentIndex"]
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
    else {
        return false;
    };
    let Some(content) = message["content"].as_array_mut() else {
        return false;
    };
    if content.len() <= index {
        content.resize(index + 1, Value::Null);
    }
    let block = &mut content[index];
    match update["type"].as_str().unwrap_or_default() {
        "text_start" => *block = json!({"type": "text", "text": ""}),
        "thinking_start" => *block = json!({"type": "thinking", "thinking": ""}),
        "toolcall_start" => {
            *block = json!({"type": "toolCall", "id": update["id"], "name": update["toolName"],
                            "arguments": {}, "partialArguments": ""});
        }
        "text_delta" => append(block, "text", update),
        "thinking_delta" => append(block, "thinking", update),
        "toolcall_delta" => append(block, "partialArguments", update),
        "text_end" => *block = json!({"type": "text", "text": update["content"]}),
        "thinking_end" => *block = json!({"type": "thinking", "thinking": update["content"]}),
        "toolcall_end" => *block = update["toolCall"].clone(),
        _ => return false,
    }
    true
}

/// Append an update's `delta` to a block's text field, in place, so a long
/// reply isn't copied on every token.
fn append(block: &mut Value, field: &str, update: &Value) {
    let delta = update["delta"].as_str().unwrap_or_default();
    match block.get_mut(field) {
        Some(Value::String(text)) => text.push_str(delta),
        _ => block[field] = Value::String(delta.to_owned()),
    }
}

/// A finished message without what no view shows: a tool's output and an
/// image's data, which can be megabytes and stay until the run settles.
fn slim(message: &Value) -> Value {
    let mut message = message.clone();
    if message["role"] == "toolResult" {
        message["content"] = json!([]);
    } else if let Some(content) = message["content"].as_array_mut() {
        for block in content.iter_mut().filter(|b| b["type"] == "image") {
            *block = json!({"type": "image"});
        }
    }
    message
}
