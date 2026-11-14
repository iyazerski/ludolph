//! Markdown in what you write to the agent. The composer and your sent
//! messages show the source as typed, shaded so its headings, lists, links
//! and code stand out; Shift+Return carries a list on.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

/// A part of the Markdown source that draws apart from plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    /// A heading's whole line.
    Heading,
    /// The syntax around the text: a heading's `#`s, a list item's bullet or
    /// number, a task's box.
    Marker,
    /// A link's text.
    Link,
    /// A link's `](target)`.
    Target,
    Code,
    Strong,
    Emphasis,
}

/// The parts of `text` that draw apart, each over the ones before it.
pub fn shades(text: &str) -> Vec<(Range<usize>, Shade)> {
    let mut out = Vec::new();
    for (event, range) in parse(text) {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                let end = range.start + text[range.clone()].trim_end().len();
                out.push((range.start..end, Shade::Heading));
                let hashes = text[range.clone()].bytes().take_while(|&b| b == b'#');
                let marker = hashes.count();
                if marker > 0 {
                    out.push((range.start..range.start + marker, Shade::Marker));
                }
            }
            Event::Start(Tag::Item) => {
                let len = marker_len(&text[range.clone()]);
                out.push((range.start..range.start + len, Shade::Marker));
            }
            Event::TaskListMarker(_) => out.push((range, Shade::Marker)),
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => {
                let source = &text[range.clone()];
                match source.rfind("](").filter(|_| source.ends_with(')')) {
                    Some(at) => {
                        out.push((range.start..range.start + at + 1, Shade::Link));
                        out.push((range.start + at + 1..range.end, Shade::Target));
                    }
                    // An autolink, `<https://…>`, or a reference link.
                    None => out.push((range, Shade::Link)),
                }
            }
            Event::Code(_) | Event::Start(Tag::CodeBlock(_)) => out.push((range, Shade::Code)),
            Event::Start(Tag::Strong) => out.push((range, Shade::Strong)),
            Event::Start(Tag::Emphasis) => out.push((range, Shade::Emphasis)),
            _ => {}
        }
    }
    out
}

/// Each link in `text`: its source range and where it points.
pub fn links(text: &str) -> Vec<(Range<usize>, String)> {
    parse(text)
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::Link { dest_url, .. }) => Some((range, dest_url.into_string())),
            _ => None,
        })
        .collect()
}

/// What Shift+Return does at byte `caret` of `text`, if the caret is in a
/// list item: the range to replace and what goes there. In an item with
/// text, it starts the next item; in an empty one, it ends the list.
pub fn list_break(text: &str, caret: usize) -> Option<(Range<usize>, String)> {
    let start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = text[caret..].find('\n').map_or(text.len(), |i| caret + i);
    let line = &text[start..end];
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let item = &line[indent..];
    let len = marker_len(item);
    if len == 0 || caret < start + indent + len {
        return None;
    }
    if item[len..].trim().is_empty() {
        return Some((start..end, String::new()));
    }
    let marker = &item[..len];
    let next = match marker.find(['.', ')']) {
        Some(at) if marker[..at].bytes().all(|b| b.is_ascii_digit()) => {
            let number: u64 = marker[..at].parse().ok()?;
            format!("{}{}", number + 1, &marker[at..at + 1])
        }
        _ => marker[..1].to_owned(),
    };
    let task = if marker.ends_with("] ") { " [ ]" } else { "" };
    Some((caret..caret, format!("\n{}{next}{task} ", &line[..indent])))
}

/// A Markdown link to `target` labelled `label`. A target with spaces, as
/// paths under Application Support have, goes in angle brackets.
pub fn link(label: &str, target: &str) -> String {
    if target.contains([' ', '(', ')']) {
        format!("[{label}](<{target}>)")
    } else {
        format!("[{label}]({target})")
    }
}

fn parse(text: &str) -> impl Iterator<Item = (Event<'_>, Range<usize>)> {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    Parser::new_ext(text, options).into_offset_iter()
}

/// The length of the list marker `item` starts with, with its space and any
/// task box: `- `, `12. `, `- [x] `. Zero if it isn't a list item.
fn marker_len(item: &str) -> usize {
    let digits = item.bytes().take_while(u8::is_ascii_digit).count();
    let bullet = match item.as_bytes() {
        [b'-' | b'*' | b'+', b' ', ..] => 2,
        _ if (1..=9).contains(&digits)
            && matches!(item.as_bytes().get(digits), Some(b'.' | b')'))
            && item.as_bytes().get(digits + 1) == Some(&b' ') =>
        {
            digits + 2
        }
        _ => return 0,
    };
    let rest = &item[bullet..];
    let task = ["[ ] ", "[x] ", "[X] "].iter().any(|b| rest.starts_with(b));
    bullet + if task { 4 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_return_carries_a_list_on() {
        // The text and caret, then the bytes replaced and what replaces them.
        let cases = [
            ("- one", 5, Some((5, 5, "\n- "))),
            ("  * one", 7, Some((7, 7, "\n  * "))),
            ("9. nine", 7, Some((7, 7, "\n10. "))),
            ("1) a", 4, Some((4, 4, "\n2) "))),
            ("- [x] done", 10, Some((10, 10, "\n- [ ] "))),
            ("intro\n- ", 8, Some((6, 8, ""))),
            ("- one", 1, None),
            ("plain", 5, None),
        ];
        for (text, caret, want) in cases {
            let want = want.map(|(start, end, s)| (start..end, s.to_owned()));
            assert_eq!(list_break(text, caret), want, "{text:?} at {caret}");
        }
    }

    #[test]
    fn markdown_source_is_shaded() {
        let text = "# Plan\n\n1. See [shot](</a b.png>) and `x`\n- **b**";
        let shaded: Vec<(&str, Shade)> = shades(text)
            .into_iter()
            .map(|(r, shade)| (&text[r], shade))
            .collect();
        assert_eq!(
            shaded,
            [
                ("# Plan", Shade::Heading),
                ("#", Shade::Marker),
                ("1. ", Shade::Marker),
                ("[shot]", Shade::Link),
                ("(</a b.png>)", Shade::Target),
                ("`x`", Shade::Code),
                ("- ", Shade::Marker),
                ("**b**", Shade::Strong),
            ]
        );
        let at = text.find("[shot]").unwrap();
        assert_eq!(links(text), [(at..at + 18, "/a b.png".to_owned())]);
        assert_eq!(link("Image 1", "/a b.png"), "[Image 1](</a b.png>)");
    }
}
