//! Text helpers shared by the chat list and the transcript.

/// `text` on one line and cut to `max` characters; `None` if it's blank.
pub(crate) fn one_line(text: &str, max: usize) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let line = words.join(" ");
    Some(match line.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    })
}
