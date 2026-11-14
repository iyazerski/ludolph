//! Normalize pi skill commands for sending and matching transcript messages.

use std::borrow::Cow;

const COMMAND: &str = "/skill:";

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
