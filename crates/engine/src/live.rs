//! pi emits no entry ids and may report messages before their file writes.
//! Match the live tail to persisted messages by role and timestamp.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::pi::Event;
use crate::skills;
use crate::transcript::{self, NestedCall, ToolState};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Live {
    revision: u64,
    working: bool,
    #[serde(skip)]
    settled: u64,
    messages: Vec<Value>,
    streaming: Option<Value>,
    tools: BTreeMap<String, Value>,
    calls: HashMap<String, Vec<NestedCall>>,
    steering: Vec<String>,
    follow_up: Vec<String>,
    retry: Option<Retry>,
    compacting: Option<String>,
    failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Retry {
    pub attempt: u32,
    pub max_attempts: u32,
    pub error: Option<String>,
    pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
    pub retry: Option<Retry>,
    pub compacting: Option<String>,
}

impl Live {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: Event) -> bool {
        let changed = self.fold(event);
        if changed {
            self.revision += 1;
        }
        changed
    }

    pub fn clear_messages(&mut self) {
        if !self.messages.is_empty() {
            self.messages.clear();
            self.revision += 1;
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Compaction can begin before prompt acceptance or `agent_start`.
    pub fn working(&self) -> bool {
        self.working || self.compacting.is_some()
    }

    pub fn compacting(&self) -> bool {
        self.compacting.is_some()
    }

    pub fn settled(&self) -> u64 {
        self.settled
    }

    /// Do not resurrect a run that settled before its prompt acknowledgement arrived.
    pub fn started(&mut self, settled: u64) -> bool {
        if self.working || self.settled != settled {
            return false;
        }
        self.working = true;
        self.failure = None;
        self.revision += 1;
        true
    }

    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    pub fn streaming(&self) -> Option<&Value> {
        self.streaming.as_ref()
    }

    pub fn tools(&self) -> &BTreeMap<String, Value> {
        &self.tools
    }

    pub fn calls(&self) -> &HashMap<String, Vec<NestedCall>> {
        &self.calls
    }

    pub fn run(&self) -> Option<Run> {
        self.working().then(|| Run {
            steering: self
                .steering
                .iter()
                .map(|text| skills::as_typed(text).into_owned())
                .collect(),
            follow_up: self
                .follow_up
                .iter()
                .map(|text| skills::as_typed(text).into_owned())
                .collect(),
            retry: self.retry.clone(),
            compacting: self.compacting.clone(),
        })
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn fold(&mut self, event: Event) -> bool {
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
                self.calls.clear();
                self.retry = None;
                self.compacting = None;
                // pi sends the queue again when it changes, but not when an
                // abort drops it.
                self.steering.clear();
                self.follow_up.clear();
            }
            Event::MessageStart { mut message } => {
                if message["role"] != "assistant" {
                    return false;
                }
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
                if message["role"] == "toolResult"
                    && let Some(id) = message["toolCallId"].as_str()
                {
                    self.calls.remove(id);
                }
                self.messages.push(slim(message));
            }
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
                parent_tool_call_id,
            } => {
                if let Some(parent) = parent_tool_call_id {
                    self.calls
                        .entry(parent)
                        .or_default()
                        .push(NestedCall::running(&tool_call_id, &tool_name, &args));
                }
                self.tools.insert(
                    tool_call_id,
                    json!({"toolName": tool_name, "args": args, "partialResult": null}),
                );
            }
            Event::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
            } => {
                let Some(tool) = self.tools.get_mut(&tool_call_id) else {
                    return false;
                };
                let calls = if tool["toolName"] == "codemode" {
                    transcript::call_progress(&partial_result["details"])
                } else {
                    Vec::new()
                };
                tool["partialResult"] = partial_result;
                if !calls.is_empty() {
                    transcript::merge_calls(self.calls.entry(tool_call_id).or_default(), calls);
                }
            }
            Event::ToolExecutionEnd {
                tool_call_id,
                parent_tool_call_id,
                is_error,
                duration_ms,
                result,
            } => {
                if let Some(parent) = parent_tool_call_id
                    && let Some(call) = self
                        .calls
                        .get_mut(&parent)
                        .and_then(|calls| calls.iter_mut().find(|call| call.id == tool_call_id))
                {
                    call.state = if is_error {
                        ToolState::Failed
                    } else {
                        ToolState::Done
                    };
                    call.duration_ms = duration_ms;
                    if is_error {
                        call.error = result["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|block| block["text"].as_str())
                            .find_map(|text| crate::text::one_line(text, 500));
                    }
                }
                return self.tools.remove(&tool_call_id).is_some();
            }
            Event::QueueUpdate {
                steering,
                follow_up,
            } => {
                self.steering = steering;
                self.follow_up = follow_up;
            }
            Event::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                error_message,
            } => {
                self.failure = None;
                let delay =
                    delay_ms.and_then(|ms| TimeDelta::try_milliseconds(i64::try_from(ms).ok()?));
                self.retry = Some(Retry {
                    attempt,
                    max_attempts,
                    error: error_message,
                    at: delay.map(|delay| Utc::now() + delay),
                });
            }
            Event::AutoRetryEnd {
                success,
                final_error,
            } => {
                self.retry = None;
                if !success {
                    self.failure = Some(final_error.unwrap_or_else(|| "the request failed".into()));
                }
            }
            Event::CompactionStart { reason } => {
                self.failure = None;
                self.compacting = Some(reason.unwrap_or_else(|| "manual".into()));
            }
            Event::CompactionEnd { error_message } => {
                self.compacting = None;
                if let Some(error) = error_message {
                    self.failure = Some(error);
                }
            }
            _ => return false,
        }
        true
    }
}

/// Block-end events replace the content accumulated from deltas.
fn update_block(message: &mut Value, mut update: Value) -> bool {
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
            *block = json!({"type": "toolCall", "id": update["id"].take(), "name": update["toolName"].take(),
                            "arguments": {}, "partialArguments": ""});
        }
        "text_delta" => append(block, "text", &update),
        "thinking_delta" => append(block, "thinking", &update),
        "toolcall_delta" => append(block, "partialArguments", &update),
        "text_end" => *block = json!({"type": "text", "text": update["content"].take()}),
        "thinking_end" => {
            *block = json!({"type": "thinking", "thinking": update["content"].take()})
        }
        "toolcall_end" => *block = update["toolCall"].take(),
        _ => return false,
    }
    true
}

/// Append deltas in place to avoid copying the reply on every token.
fn append(block: &mut Value, field: &str, update: &Value) {
    let delta = update["delta"].as_str().unwrap_or_default();
    match block.get_mut(field) {
        Some(Value::String(text)) => text.push_str(delta),
        _ => block[field] = Value::String(delta.to_owned()),
    }
}

/// Keep bulky output and image bytes in the session file, not the live tail.
fn slim(mut message: Value) -> Value {
    if message["role"] == "toolResult" {
        message["content"] = json!([]);
    } else if let Some(content) = message["content"].as_array_mut() {
        for block in content.iter_mut().filter(|b| b["type"] == "image") {
            *block = json!({"type": "image"});
        }
    }
    message
}
