use chrono::{DateTime, Utc};

use crate::data::{NestedCall, Spent, ToolState};

pub fn ago(when: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let s = (now - when).num_seconds().max(0);
    if s < 60 {
        "now".to_owned()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else if s < 604_800 {
        format!("{}d", s / 86_400)
    } else {
        format!("{}w", s / 604_800)
    }
}

pub fn worked(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, s % 3600 / 60)
    }
}

pub fn context_readout(tokens: Option<u64>, window: u64, spent: Option<Spent>) -> (String, String) {
    let used = tokens.map_or_else(|| "—".to_owned(), thousands);
    let cost = spent.map_or_else(|| "—".to_owned(), |spent| dollars(spent.cost));
    (format!("{used}/{}", thousands(window)), cost)
}

pub fn dollars(cost: f64) -> String {
    if cost > 0.0 && cost < 0.01 {
        "<$0.01".to_owned()
    } else {
        let rounded = (cost * 100.0).round() / 100.0;
        format!("${rounded:.2}")
    }
}

fn thousands(tokens: u64) -> String {
    format!("{}k", (tokens as f64 / 100.0).round() / 10.0)
}

pub fn short_model(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

pub fn reasoning_name(level: &str) -> String {
    if level == "xhigh" {
        return "X-High".to_owned();
    }
    let mut chars = level.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

pub fn tone_index(path: &str) -> usize {
    let hash = path.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    (hash % 8) as usize
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolCall<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub subject: &'a str,
    pub state: ToolState,
    pub calls: &'a [NestedCall],
}

impl<'a> From<&'a NestedCall> for ToolCall<'a> {
    fn from(call: &'a NestedCall) -> Self {
        Self {
            id: &call.id,
            name: &call.name,
            subject: call.subject.as_deref().unwrap_or_default(),
            state: call.state,
            calls: &[],
        }
    }
}

impl ToolCall<'_> {
    fn known(&self) -> bool {
        matches!(
            self.name,
            "bash" | "read" | "write" | "edit" | "grep" | "find" | "ls"
        )
    }

    pub fn verb(&self) -> &'static str {
        match self.name {
            "bash" => "Run",
            "read" => "Read",
            "write" => "Write",
            "edit" => "Edit",
            "grep" => "Search",
            "find" => "Glob",
            "ls" => "List",
            _ => "Tool",
        }
    }

    pub fn arg(&self) -> String {
        if self.known() {
            self.subject.to_owned()
        } else if self.subject.is_empty() {
            self.name.to_owned()
        } else {
            format!("{} · {}", self.name, self.subject)
        }
    }

    pub fn on_file(&self) -> bool {
        matches!(self.name, "read" | "write" | "edit") && !self.subject.is_empty()
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn tool_summary(tools: &[ToolCall]) -> String {
    let scripts = tools.iter().filter(|tool| tool.name == "codemode").count();
    let tools: Vec<_> = tools
        .iter()
        .flat_map(|tool| {
            std::iter::once(*tool)
                .filter(|tool| tool.calls.is_empty() && tool.name != "codemode")
                .chain(tool.calls.iter().map(ToolCall::from))
        })
        .collect();
    if tools.is_empty() && scripts > 0 {
        return format!("Ran {}", plural(scripts, "script", "scripts"));
    }
    let mut edited: Vec<&str> = Vec::new();
    let (mut commands, mut reads, mut searches, mut fetches, mut todos, mut other) =
        (0, 0, 0, 0, 0, 0);
    for t in &tools {
        if t.state == ToolState::Cancelled {
            continue;
        }
        match t.name {
            "bash" => commands += 1,
            "write" | "edit" => {
                let file = if t.subject.is_empty() {
                    t.id
                } else {
                    t.subject
                };
                if !edited.contains(&file) {
                    edited.push(file);
                }
            }
            "read" => reads += 1,
            "grep" | "find" | "ls" => searches += 1,
            name if name.contains("fetch") => fetches += 1,
            name if name.contains("todo") => todos += 1,
            _ => other += 1,
        }
    }
    let cancelled = tools
        .iter()
        .filter(|t| t.state == ToolState::Cancelled)
        .count();
    let failed = tools
        .iter()
        .filter(|t| t.state == ToolState::Failed)
        .count();
    let mut parts = Vec::new();
    let mut add = |n: usize, word: &str, one: &str, many: &str| {
        if n > 0 {
            parts.push(format!("{word} {}", plural(n, one, many)));
        }
    };
    add(commands, "ran", "command", "commands");
    add(edited.len(), "edited", "file", "files");
    add(reads, "read", "file", "files");
    add(searches, "searched", "time", "times");
    add(fetches, "fetched", "page", "pages");
    if todos > 0 {
        parts.push("updated todos".to_owned());
    }
    if other > 0 {
        parts.push(format!("called {}", plural(other, "tool", "tools")));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if cancelled > 0 {
        parts.push(format!("{cancelled} cancelled"));
    }
    let s = parts.join(" · ");
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
