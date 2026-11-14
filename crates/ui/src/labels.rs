//! How the windows word what they show, as the phone app words it: ages,
//! model and thinking names, tool calls.

use chrono::{DateTime, Utc};

use crate::data::{Spent, ToolState};

/// Compact age, like the desktop sidebar: now, 34m, 4h, 2d, 3w.
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

/// How long a turn worked: 42s, 6m 56s, 1h 5m.
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

/// Format the context's used/capacity counts and cumulative chat cost.
pub fn context_readout(tokens: Option<u64>, window: u64, spent: Option<Spent>) -> (String, String) {
    let used = tokens.map_or_else(|| "—".to_owned(), thousands);
    let cost = spent.map_or_else(
        || "—".to_owned(),
        |spent| {
            if spent.cost > 0.0 && spent.cost < 0.01 {
                "<$0.01".to_owned()
            } else {
                let rounded = (spent.cost * 100.0).round() / 100.0;
                format!("${rounded:.2}")
            }
        },
    );
    (format!("{used}/{}", thousands(window)), cost)
}

/// Round a token count to tenths of a thousand without trailing zeroes.
fn thousands(tokens: u64) -> String {
    format!("{}k", (tokens as f64 / 100.0).round() / 10.0)
}

/// A model id without its provider: `anthropic/claude-x` reads `claude-x`.
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

/// The desktop's project tone, by FNV-1a of the project path.
pub fn tone_index(path: &str) -> usize {
    let hash = path.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    (hash % 8) as usize
}

/// One tool call, as a message's tool part has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCall<'a> {
    pub id: &'a str,
    /// pi's name for the tool.
    pub name: &'a str,
    pub subject: &'a str,
    pub state: ToolState,
}

impl ToolCall<'_> {
    /// Whether this is one of pi's own tools, which the rows name by verb.
    fn known(&self) -> bool {
        matches!(
            self.name,
            "bash" | "read" | "write" | "edit" | "grep" | "find" | "ls"
        )
    }

    /// The verb a row starts with ("Run cargo test").
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

    /// What a row names after its verb: the subject, with an extension's
    /// tool named first.
    pub fn arg(&self) -> String {
        if self.known() {
            self.subject.to_owned()
        } else if self.subject.is_empty() {
            self.name.to_owned()
        } else {
            format!("{} · {}", self.name, self.subject)
        }
    }

    /// Whether the subject is a file, shown by name on a badge.
    pub fn on_file(&self) -> bool {
        matches!(self.name, "read" | "write" | "edit") && !self.subject.is_empty()
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A tool group's one-line summary, as the phone words it.
pub fn tool_summary(tools: &[ToolCall]) -> String {
    let mut edited: Vec<&str> = Vec::new();
    let (mut commands, mut reads, mut searches, mut fetches, mut todos, mut other) =
        (0, 0, 0, 0, 0, 0);
    for t in tools {
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
    let s = parts.join(" · ");
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_readout_is_compact_and_keeps_unknown_usage_explicit() {
        for (tokens, cost, want) in [
            (Some(75_000), Some(0.75), ("75k/272k", "$0.75")),
            (Some(1500), Some(0.625), ("1.5k/272k", "$0.63")),
            (Some(0), Some(0.0), ("0k/272k", "$0.00")),
            (None, Some(0.001), ("—/272k", "<$0.01")),
            (None, None, ("—/272k", "—")),
        ] {
            let spent = cost.map(|cost| Spent { cost });
            let got = context_readout(tokens, 272_000, spent);
            assert_eq!((got.0.as_str(), got.1.as_str()), want);
        }
    }

    #[test]
    fn tool_groups_summarize_like_the_phone() {
        let call = |name, subject, state| ToolCall {
            id: "t",
            name,
            subject,
            state,
        };
        let tools = [
            call("bash", "ls", ToolState::Done),
            call("bash", "pwd", ToolState::Done),
            call("edit", "a.rs", ToolState::Done),
            call("edit", "a.rs", ToolState::Done),
            call("read", "b.rs", ToolState::Done),
            call("codemode", "", ToolState::Failed),
        ];
        assert_eq!(
            tool_summary(&tools),
            "Ran 2 commands · edited 1 file · read 1 file · called 1 tool · 1 failed"
        );
        assert_eq!(tools[5].arg(), "codemode");
        assert_eq!(call("mcp", "x", ToolState::Done).arg(), "mcp · x");
    }

    #[test]
    fn ages_read_like_the_sidebar() {
        let now: DateTime<Utc> = "2026-11-06T12:00:00Z".parse().unwrap();
        for (secs, want) in [
            (5, "now"),
            (125, "2m"),
            (7300, "2h"),
            (90_000, "1d"),
            (1_300_000, "2w"),
        ] {
            assert_eq!(ago(now - chrono::Duration::seconds(secs), now), want);
        }
    }

    #[test]
    fn work_times_read_in_minutes_and_seconds() {
        for (secs, want) in [(42, "42s"), (416, "6m 56s"), (3900, "1h 5m")] {
            assert_eq!(worked(secs), want);
        }
    }
}
