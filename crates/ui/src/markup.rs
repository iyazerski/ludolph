use std::ops::Range;

use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    Heading,
    Marker,
    Link,
    WebUrl,
    Target,
    Code,
    Strong,
    Emphasis,
    Skill,
}

#[derive(Debug, Default)]
pub struct Markup {
    pub shades: Vec<(Range<usize>, Shade)>,
    pub links: Vec<(Range<usize>, String)>,
}

pub fn analyze(text: &str) -> Markup {
    let mut markup = Markup::default();
    let out = &mut markup.shades;
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    let autolink = text.contains("://");
    let mut protected = 0;
    let mut events = parse(text).peekable();
    while let Some((event, mut range)) = events.next() {
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
            Event::Start(tag @ (Tag::Link { .. } | Tag::Image { .. })) => {
                protected += 1;
                if let Tag::Link { dest_url, .. } = tag {
                    markup.links.push((range.clone(), dest_url.into_string()));
                }
                let source = &text[range.clone()];
                match source.rfind("](").filter(|_| source.ends_with(')')) {
                    Some(at) => {
                        out.push((range.start..range.start + at + 1, Shade::Link));
                        out.push((range.start + at + 1..range.end, Shade::Target));
                    }
                    None => out.push((range, Shade::Link)),
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                protected += 1;
                out.push((range, Shade::Code));
            }
            Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => protected -= 1,
            Event::Text(_) if autolink && protected == 0 => {
                // Pulldown can split plain text at punctuation inside a single URL.
                while let Some((Event::Text(_), next)) = events.peek() {
                    if next.start != range.end {
                        break;
                    }
                    range.end = next.end;
                    events.next();
                }
                for link in finder
                    .links(&text[range.clone()])
                    .filter(|l| is_web_url(l.as_str()))
                {
                    let span = range.start + link.start()..range.start + link.end();
                    out.push((span.clone(), Shade::WebUrl));
                    markup.links.push((span, link.as_str().to_owned()));
                }
            }
            Event::Code(_) => out.push((range, Shade::Code)),
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
    markup
}

pub fn is_web_url(text: &str) -> bool {
    text.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
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
