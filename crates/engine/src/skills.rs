//! Skills for the composer's `$` menu, and the `/skill:name` commands that
//! run them.
//!
//! pi runs a skill when a prompt starts with `/skill:name`: it replaces the
//! command with a `<skill>` block holding the skill's `SKILL.md`, followed by
//! the rest of the prompt, and saves that as the user message.

use std::borrow::Cow;

use serde_json::Value;

/// What starts a skill command.
const COMMAND: &str = "/skill:";

/// One skill pi can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The name after `/skill:`.
    pub name: String,
    pub description: Option<String>,
}

impl Skill {
    /// The command that runs it, which the composer puts in the prompt.
    pub fn command(&self) -> String {
        format!("{COMMAND}{}", self.name)
    }
}

/// The skills among the commands of pi's `get_commands` answer.
pub fn from_commands(answer: &Value) -> Vec<Skill> {
    let commands = answer["commands"].as_array().map_or(&[][..], Vec::as_slice);
    commands
        .iter()
        .filter(|command| command["source"] == "skill")
        .filter_map(|command| {
            let name = command["name"].as_str()?.strip_prefix("skill:")?;
            Some(Skill {
                name: name.to_owned(),
                description: command["description"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

/// `text` with its first `/skill:name` moved to the front, followed by a
/// space, since pi runs a skill only from there and reads its name up to the
/// first space. The composer puts the command where the `$name` was typed.
/// Later commands stay where they are, as text.
pub fn to_front(text: &str) -> Cow<'_, str> {
    if text.starts_with(COMMAND) {
        return Cow::Borrowed(text);
    }
    let found = text
        .match_indices(COMMAND)
        .map(|(at, _)| at)
        .find(|&at| text[..at].ends_with(char::is_whitespace));
    let Some(at) = found else {
        return Cow::Borrowed(text);
    };
    let end = text[at..]
        .find(char::is_whitespace)
        .map_or(text.len(), |len| at + len);
    if end == at + COMMAND.len() {
        return Cow::Borrowed(text);
    }
    // Of the gaps around the command, the one with more line breaks stays,
    // so paragraphs do.
    let before = text[..at].trim_end();
    let after = text[end..].trim_start();
    let (gap_before, gap_after) = (
        &text[before.len()..at],
        &text[end..text.len() - after.len()],
    );
    let lines = |gap: &str| gap.matches('\n').count();
    let gap = if lines(gap_before) > lines(gap_after) {
        gap_before
    } else {
        gap_after
    };
    let rest = format!("{before}{gap}{after}");
    let rest = rest.trim();
    let command = &text[at..end];
    Cow::Owned(if rest.is_empty() {
        command.to_owned()
    } else {
        format!("{command} {rest}")
    })
}

/// A user message's text as it was typed: a skill pi expanded goes back to
/// `/skill:name` and the request, as pi's own view shows it.
pub fn as_typed(text: &str) -> Cow<'_, str> {
    match expanded(text) {
        Some((name, "")) => Cow::Owned(format!("{COMMAND}{name}")),
        Some((name, request)) => Cow::Owned(format!("{COMMAND}{name} {request}")),
        None => Cow::Borrowed(text),
    }
}

/// The skill's name and the request after it, if `text` is an expanded
/// skill. It follows `parseSkillBlock` in pi's `agent-session.js`.
fn expanded(text: &str) -> Option<(&str, &str)> {
    let (head, body) = text.split_once('\n')?;
    let attributes = head.strip_prefix("<skill name=\"")?.strip_suffix("\">")?;
    let (name, _) = attributes.split_once("\" location=\"")?;
    // The first end tag that the request, if any, follows.
    let request = body.match_indices("</skill>").find_map(|(at, _)| {
        body[..at].strip_suffix('\n')?;
        let after = &body[at + "</skill>".len()..];
        if after.is_empty() {
            Some("")
        } else {
            after.strip_prefix("\n\n")
        }
    })?;
    Some((name, request.trim()))
}
