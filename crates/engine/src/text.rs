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

/// Terminal output as plain text: no color codes or other control characters, and of the lines a
/// progress bar redraws with `\r`, only the last.
pub(crate) fn plain(output: &str) -> String {
    let mut text = String::with_capacity(output.len());
    let mut chars = output.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            text.push(c);
        } else if chars.next() == Some('[') {
            // A CSI sequence ends at its first byte in @..=~.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    text.lines()
        .map(|line| {
            let line = line
                .rsplit('\r')
                .find(|part| !part.is_empty())
                .unwrap_or("");
            line.replace(|c: char| c.is_control() && c != '\t', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
