use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    Heading,
    Marker,
    Link,
    Target,
    Code,
    Strong,
    Emphasis,
    Skill,
}

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
                    None => out.push((range, Shade::Link)),
                }
            }
            Event::Code(_) | Event::Start(Tag::CodeBlock(_)) => out.push((range, Shade::Code)),
            Event::Start(Tag::Strong) => out.push((range, Shade::Strong)),
            Event::Start(Tag::Emphasis) => out.push((range, Shade::Emphasis)),
            _ => {}
        }
    }
    let mut at = 0;
    for (piece, skill) in ludolph_prompt::skill_words(text) {
        let end = at + piece.len();
        if skill {
            out.push((at..end, Shade::Skill));
        }
        at = end;
    }
    out
}

pub fn links(text: &str) -> Vec<(Range<usize>, String)> {
    parse(text)
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::Link { dest_url, .. }) => Some((range, dest_url.into_string())),
            _ => None,
        })
        .collect()
}

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
