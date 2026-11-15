fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '-')
}

/// Return the byte offset of `$` and the lowercased query ending at the caret.
pub fn skill_query(text: &str, caret: usize) -> Option<(usize, String)> {
    let before = text.get(..caret)?;
    let start = before.rfind('$')?;
    let word = &before[start + 1..];
    let lead = before[..start].chars().next_back();
    let ok = lead.is_none_or(char::is_whitespace) && word.chars().all(name_char);
    ok.then(|| (start, word.to_lowercase()))
}

/// Return (piece, is_skill) pairs without moving text; prices such as $5 are not skills.
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
