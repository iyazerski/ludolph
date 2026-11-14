//! Discover pi skills, load inline mentions, and keep their source text in chat views.

use std::borrow::Cow;
use std::fmt::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::Value;

/// One skill pi can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub path: PathBuf,
}

impl Skill {
    /// Return the inline reference the composer inserts.
    pub fn reference(&self) -> String {
        format!("${}", self.name)
    }
}

/// Read skills and their instruction paths from pi's command discovery.
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
                path: command["sourceInfo"]["path"].as_str()?.into(),
            })
        })
        .collect()
}

/// Load each known inline mention without moving any surrounding text.
///
/// Fail if a referenced skill's instructions cannot be read.
pub fn expand(text: &str, skills: &[Skill]) -> Result<String> {
    let mut out = String::new();
    for (piece, mention) in ludolph_prompt::skill_words(text) {
        let skill = mention
            .then(|| skills.iter().find(|skill| piece[1..] == skill.name))
            .flatten();
        let Some(skill) = skill else {
            out.push_str(piece);
            continue;
        };
        let content = std::fs::read_to_string(&skill.path)
            .with_context(|| format!("reading skill {} at {}", skill.name, skill.path.display()))?;
        let directory = skill
            .path
            .parent()
            .context("a skill has no parent folder")?;
        write!(
            out,
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
            skill.name,
            skill.path.display(),
            directory.display(),
            content.trim(),
        )?;
    }
    Ok(out)
}

/// Collapse expanded skill instructions to inline mentions, preserving surrounding text.
pub fn as_typed(text: &str) -> Cow<'_, str> {
    let mut rest = text;
    let mut out = String::new();
    let mut changed = false;
    while let Some(at) = rest.find("<skill name=\"") {
        let (before, candidate) = rest.split_at(at);
        out.push_str(before);
        if let Some((name, end)) = expanded(candidate) {
            out.push('$');
            out.push_str(name);
            rest = &candidate[end..];
            changed = true;
        } else {
            out.push('<');
            rest = &candidate[1..];
        }
    }
    if changed {
        out.push_str(rest);
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Read a pi-style instruction block and its end offset.
fn expanded(text: &str) -> Option<(&str, usize)> {
    let (head, body) = text.split_once('\n')?;
    let attributes = head.strip_prefix("<skill name=\"")?.strip_suffix("\">")?;
    let (name, _) = attributes.split_once("\" location=\"")?;
    let closing = "\n</skill>";
    let end = head.len() + 1 + body.find(closing)? + closing.len();
    Some((name, end))
}
