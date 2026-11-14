//! Compact nested-call rows from pi's recorded calls and codemode summaries.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use super::{Arguments, TOOL_CHARS, ToolState, tool_subject};
use crate::text::one_line;

/// A tool invoked inside another tool, without its potentially large output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedCall {
    pub id: String,
    pub name: String,
    pub subject: Option<String>,
    pub state: ToolState,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
}

impl NestedCall {
    /// Describe a running call from its execution event.
    pub(crate) fn running(id: &str, name: &str, args: &Value) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            subject: subject(name, args),
            state: ToolState::Running,
            duration_ms: None,
            error: None,
        }
    }
}

/// Only the small call metadata, skipping all other tool details.
#[derive(Debug, Default, Deserialize)]
pub(super) struct Calls {
    #[serde(default)]
    calls: Vec<Call>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Call {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    arguments: Value,
    args: Option<String>,
    duration_ms: Option<f64>,
    error: Option<String>,
}

/// Combine recorded arguments with codemode's summaries, including model calls.
pub(super) fn recorded(record: Option<&Calls>, details: Option<&Calls>) -> Vec<NestedCall> {
    let mut records = record.map_or_else(Vec::new, |record| convert(record, false));
    let mut calls = details.map_or_else(Vec::new, |details| convert(details, false));
    for call in &mut calls {
        if let Some(at) = records.iter().position(|record| record.id == call.id) {
            let record = records.remove(at);
            call.subject = record.subject.or_else(|| call.subject.take());
            call.duration_ms = call.duration_ms.or(record.duration_ms);
            call.error = call.error.take().or(record.error);
        }
    }
    calls.extend(records);
    calls
}

/// Read codemode's live summaries without retaining its output.
pub(crate) fn progress(details: &Value) -> Vec<NestedCall> {
    Calls::deserialize(details)
        .map(|calls| convert(&calls, true))
        .unwrap_or_default()
}

/// Update calls by id while preserving launch order and richer event subjects.
pub(crate) fn merge(calls: &mut Vec<NestedCall>, updates: Vec<NestedCall>) {
    for update in updates {
        if let Some(call) = calls.iter_mut().find(|call| call.id == update.id) {
            if update.subject.is_some() && call.subject.is_none() {
                call.subject = update.subject;
            }
            call.state = update.state;
            call.duration_ms = update.duration_ms.or(call.duration_ms);
            call.error = update.error.or_else(|| call.error.take());
        } else {
            calls.push(update);
        }
    }
}

fn convert(calls: &Calls, live: bool) -> Vec<NestedCall> {
    calls
        .calls
        .iter()
        // Execution events cover provisional /? calls until pi assigns their real ids.
        .filter(|call| !call.id.ends_with("/?"))
        .map(|call| {
            let args = call
                .args
                .as_deref()
                .and_then(|args| serde_json::from_str(args).ok());
            NestedCall {
                id: call.id.clone(),
                name: call.name.clone(),
                subject: subject(&call.name, &call.arguments)
                    .or_else(|| args.as_ref().and_then(|args| subject(&call.name, args)))
                    .or_else(|| {
                        call.args
                            .as_deref()
                            .and_then(|args| one_line(args, TOOL_CHARS))
                    }),
                state: match call.status.as_str() {
                    "ok" => ToolState::Done,
                    "error" => ToolState::Failed,
                    "running" | "unfinished" if live => ToolState::Running,
                    _ => ToolState::Cancelled,
                },
                duration_ms: call.duration_ms,
                error: call.error.as_deref().and_then(|error| one_line(error, 500)),
            }
        })
        .collect()
}

fn subject(name: &str, args: &Value) -> Option<String> {
    if args.is_null() {
        return None;
    }
    let parsed = Arguments::deserialize(args).ok();
    tool_subject(name, parsed.as_ref()).or_else(|| one_line(&args.to_string(), TOOL_CHARS))
}
