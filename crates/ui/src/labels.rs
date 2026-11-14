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

/// The context ring's tooltip: how full the context is, and what the chat
/// has spent so far.
pub fn context_help(tokens: Option<u64>, window: u64, spent: Option<Spent>) -> String {
    let mut help = match tokens {
        Some(tokens) => format!(
            "Context: {} of {} tokens ({}%)\n{} tokens left",
            count(tokens),
            count(window),
            (tokens as f64 / window as f64 * 100.0).round(),
            count(window.saturating_sub(tokens)),
        ),
        // pi can't tell until the first answer after a compaction.
        None => format!(
            "Context: {} tokens\nSize known after the next answer",
            count(window)
        ),
    };
    match spent.filter(|spent| spent.tokens > 0) {
        Some(spent) if spent.cost > 0.0 => help.push_str(&format!(
            "\nThis chat: {} tokens, {}",
            count(spent.tokens),
            dollars(spent.cost)
        )),
        Some(spent) => help.push_str(&format!("\nThis chat: {} tokens", count(spent.tokens))),
        None => {}
    }
    help
}

/// A count with thousands separators: 1,234,567.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// US dollars to the cent, with a floor so a tiny cost doesn't read as free.
fn dollars(cost: f64) -> String {
    if cost < 0.01 {
        "under $0.01".to_owned()
    } else {
        format!("${cost:.2}")
    }
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
    fn context_help_reads_the_context_and_the_spend() {
        let spent = |tokens, cost| Some(Spent { tokens, cost });
        let cases = [
            (
                Some(96_000),
                spent(1_234_567, 0.454),
                "Context: 96,000 of 200,000 tokens (48%)\n104,000 tokens left\nThis chat: 1,234,567 tokens, $0.45",
            ),
            (
                None,
                spent(512, 0.001),
                "Context: 200,000 tokens\nSize known after the next answer\nThis chat: 512 tokens, under $0.01",
            ),
            (
                Some(0),
                spent(900, 0.0),
                "Context: 0 of 200,000 tokens (0%)\n200,000 tokens left\nThis chat: 900 tokens",
            ),
            (
                Some(1_000),
                spent(0, 0.0),
                "Context: 1,000 of 200,000 tokens (1%)\n199,000 tokens left",
            ),
        ];
        for (tokens, spent, want) in cases {
            assert_eq!(context_help(tokens, 200_000, spent), want);
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
