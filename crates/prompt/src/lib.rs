//! Find inline `$skill` mentions without changing the user's wording or order.

/// Whether `c` can be in a skill's name after the `$`.
fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '-')
}

/// The `$` word the caret at byte `caret` ends: where its `$` is, and the
/// lowercased text after it.
pub fn skill_query(text: &str, caret: usize) -> Option<(usize, String)> {
    let before = text.get(..caret)?;
    let start = before.rfind('$')?;
    let word = &before[start + 1..];
    let lead = before[..start].chars().next_back();
    let ok = lead.is_none_or(char::is_whitespace) && word.chars().all(name_char);
    ok.then(|| (start, word.to_lowercase()))
}

/// `text` cut at its `$name` words, as the engine shows a skill that ran:
/// `(piece, is_skill)`. A name starts with a letter, so prices stay text.
pub fn skill_words(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let (mut plain, mut at) = (0, 0);
    while let Some(i) = text[at..].find('$').map(|i| i + at) {
        let lead_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let after = &text[i + 1..];
        let len = after.find(|c| !name_char(c)).unwrap_or(after.len());
        let name = after[..len].trim_end_matches(':');
        if lead_ok && name.starts_with(char::is_alphabetic) {
            if i > plain {
                out.push((&text[plain..i], false));
            }
            let end = i + 1 + name.len();
            out.push((&text[i..end], true));
            (plain, at) = (end, end);
        } else {
            at = i + 1;
        }
    }
    if plain < text.len() {
        out.push((&text[plain..], false));
    }
    out
}
