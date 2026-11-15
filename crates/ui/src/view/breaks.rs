//! Rune wraps Markdown only at spaces. Add zero-width break points for display, never to copied text.

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

use super::code::MARKDOWN;

pub const ZWSP: char = '\u{200B}';
const LONG: usize = 40;
const RUN: usize = 24;
const BREAK_AFTER: &[char] = &['/', '.', '?', '=', '-', '#', ':', ',', '+', '@', '%'];
/// Markdown's markup characters: a break point next to one could change how
/// the text parses (`__init__`, `\|`, `&amp;`), so none goes there.
const MARKUP: &[char] = &[
    '\\', '`', '*', '_', '~', '[', ']', '(', ')', '<', '>', '|', '&', ';', '!',
];

pub fn breakable(src: &str) -> Option<String> {
    if !src.split_whitespace().any(|w| w.len() > LONG) {
        return None;
    }
    let mut kept: Vec<Range<usize>> = Vec::new();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut label: Option<Vec<Range<usize>>> = None;
    let mut image = 0usize;
    for (event, range) in Parser::new_ext(src, MARKDOWN).into_offset_iter() {
        match event {
            Event::Start(Tag::Link {
                link_type: LinkType::Autolink,
                dest_url,
                ..
            }) if dest_url.len() > LONG => {
                edits.push((range.clone(), link(&dest_url, &dest_url)));
                kept.push(range);
            }
            // Its text is its address, which must stay whole.
            Event::Start(Tag::Link {
                link_type: LinkType::Autolink | LinkType::Email,
                ..
            }) => kept.push(range),
            Event::Start(Tag::Link { .. }) => {
                kept.push(range);
                label = Some(Vec::new());
            }
            Event::End(TagEnd::Link) => {
                for text in merged(label.take().unwrap_or_default()) {
                    soften_tokens(src, text, &mut edits);
                }
            }
            Event::Start(Tag::Image { .. }) => {
                kept.push(range);
                image += 1;
            }
            Event::End(TagEnd::Image) => image = image.saturating_sub(1),
            Event::Text(_) if image == 0 => {
                if let Some(label) = label.as_mut() {
                    label.push(range);
                }
            }
            Event::Code(_) if image == 0 => {
                soften_tokens(src, range.clone(), &mut edits);
                kept.push(range);
            }
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::MetadataBlock(_))
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Rule => kept.push(range),
            _ => {}
        }
    }
    let mut at = 0;
    for span in merged(kept) {
        if at < span.start {
            prose(src, at..span.start, &mut edits);
        }
        at = at.max(span.end);
    }
    if at < src.len() {
        prose(src, at..src.len(), &mut edits);
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(src.len() + src.len() / 8);
    let mut at = 0;
    for (range, with) in edits {
        out.push_str(&src[at..range.start]);
        out.push_str(&with);
        at = range.end;
    }
    out.push_str(&src[at..]);
    Some(out)
}

fn prose(src: &str, range: Range<usize>, edits: &mut Vec<(Range<usize>, String)>) {
    for token in tokens(src, range) {
        let word = &src[token.clone()];
        let line = src[..token.start].rsplit('\n').next().unwrap_or_default();
        // A reference definition (`[x]: https://…`) keeps its address whole.
        if line.trim_start().starts_with('[') && line.contains("]:") {
            continue;
        }
        match bare_url(word) {
            Some(url) => {
                let shown = &word[url.clone()];
                let dest = if shown.starts_with("www.") {
                    format!("http://{shown}")
                } else {
                    shown.to_owned()
                };
                let at = token.start + url.start..token.start + url.end;
                edits.push((at, link(shown, &dest)));
            }
            None => soften_tokens(src, token, edits),
        }
    }
}

fn bare_url(word: &str) -> Option<Range<usize>> {
    let start = ["https://", "http://", "www."]
        .iter()
        .filter_map(|p| word.find(p))
        .min()?;
    if !word[..start].chars().all(|c| "*_~([".contains(c)) {
        return None;
    }
    let mut end = word.len();
    loop {
        let url = &word[start..end];
        let opened = url.matches('(').count() >= url.matches(')').count();
        match url.chars().next_back() {
            Some('?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"') => end -= 1,
            Some(')') if !opened => end -= 1,
            _ => break,
        }
    }
    (end - start > LONG / 2).then_some(start..end)
}

fn link(shown: &str, dest: &str) -> String {
    let mut text = String::with_capacity(shown.len() + 16);
    for c in soften(shown).chars() {
        if MARKUP.contains(&c) {
            text.push('\\');
        }
        text.push(c);
    }
    format!("[{text}](<{dest}>)")
}

fn soften_tokens(src: &str, range: Range<usize>, edits: &mut Vec<(Range<usize>, String)>) {
    for token in tokens(src, range) {
        let soft = soften(&src[token.clone()]);
        if soft.len() > token.len() {
            edits.push((token, soft));
        }
    }
}

fn tokens(src: &str, range: Range<usize>) -> impl Iterator<Item = Range<usize>> + '_ {
    let base = range.start;
    let text = &src[range];
    let mut start = None;
    text.char_indices()
        .map(Some)
        .chain([None])
        .filter_map(move |next| match (next, start) {
            (Some((i, c)), None) if !c.is_whitespace() => {
                start = Some(i);
                None
            }
            (Some((i, c)), Some(from)) if c.is_whitespace() => {
                start = None;
                Some(base + from..base + i)
            }
            (None, Some(from)) => Some(base + from..base + text.len()),
            _ => None,
        })
        .filter(|r| r.len() > LONG)
        // A rule or underline of markup has nothing to break.
        .filter(move |r| src[r.clone()].chars().any(char::is_alphanumeric))
}

fn soften(token: &str) -> String {
    let mut out = String::with_capacity(token.len() + token.len() / 8);
    let mut run = 0;
    let mut chars = token.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        run += 1;
        let Some(&next) = chars.peek() else { break };
        if MARKUP.contains(&c) || MARKUP.contains(&next) {
            continue;
        }
        if BREAK_AFTER.contains(&c) || run >= RUN {
            out.push(ZWSP);
            run = 0;
        }
    }
    out
}

fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/anomalyco/rune/pull/1234/files#diff-0a1b2c3d";

    #[test]
    fn breaks_long_tokens_only() {
        let path = "/Users/me/projects/ludolph/crates/ui/src/view/chat.rs";
        let cases: &[(String, Option<String>)] = &[
            ("short words only".into(), None),
            (
                format!("see {URL}."),
                Some(format!("see {}.", link(URL, URL))),
            ),
            (
                format!("**{URL}**"),
                Some(format!("**{}**", link(URL, URL))),
            ),
            (format!("<{URL}>"), Some(link(URL, URL))),
            (
                format!("[{URL}]({URL})"),
                Some(format!("[{}]({URL})", soften(URL))),
            ),
            (
                format!("open `{path}`"),
                Some(format!("open `{}`", soften(path))),
            ),
            (format!("```\n{path}\n```\n"), None),
            (format!("[x]: {URL}\n"), None),
        ];
        for (src, want) in cases {
            assert_eq!(&breakable(src), want, "source {src:?}");
        }
    }
}
