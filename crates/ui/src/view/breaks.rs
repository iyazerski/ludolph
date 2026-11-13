//! Break points for long links, paths and other unbroken tokens in the
//! transcript's Markdown. Rune's Markdown wraps lines only at spaces, so a
//! token wider than the window runs past its edge and gets cut off. Zero-width
//! spaces inside such tokens give the line places to break. The source the
//! app keeps, copies and sends stays as it was.

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

use super::code::MARKDOWN;

/// The break point: drawn as nothing, a place a line may wrap.
pub const ZWSP: char = '\u{200B}';
/// Tokens up to this many bytes fit a narrow window, so stay as written.
const LONG: usize = 40;
/// The most characters between break points when a token has none of its own.
const RUN: usize = 24;
/// Characters a long token breaks after, as a browser breaks a URL.
const BREAK_AFTER: &[char] = &['/', '.', '?', '=', '-', '#', ':', ',', '+', '@', '%'];
/// Markdown's markup characters: a break point next to one could change how
/// the text parses (`__init__`, `\|`, `&amp;`), so none goes there.
const MARKUP: &[char] = &[
    '\\', '`', '*', '_', '~', '[', ']', '(', ')', '<', '>', '|', '&', ';', '!',
];

/// `src` with break points in its long tokens, or `None` when it has none.
/// A bare URL becomes a link whose text breaks, so its address stays whole.
pub fn breakable(src: &str) -> Option<String> {
    if !src.split_whitespace().any(|w| w.len() > LONG) {
        return None;
    }
    // Spans left as written, and replacements, by source range.
    let mut kept: Vec<Range<usize>> = Vec::new();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    // The text ranges of the link being read, to break as its label.
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

/// Break the long tokens of prose `range`, making its bare URLs links.
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

/// Where in `word` a bare URL is, as GFM finds one: after markup that may
/// open it, without the punctuation that may close the sentence.
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

/// A link to `dest` whose text is `shown` with break points.
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

/// Break the long tokens of `range`, as written.
fn soften_tokens(src: &str, range: Range<usize>, edits: &mut Vec<(Range<usize>, String)>) {
    for token in tokens(src, range) {
        let soft = soften(&src[token.clone()]);
        if soft.len() > token.len() {
            edits.push((token, soft));
        }
    }
}

/// The source ranges of `range`'s tokens too long to fit, without spaces.
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

/// `token` with a break point after each of its URL punctuation, and at least
/// every [`RUN`] characters.
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

/// `ranges` sorted, with touching and overlapping ones joined.
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

    #[test]
    fn breaks_keep_the_text_and_markup() {
        let soft = soften("a_very__long_name::with_markup_and/no/breaks_next_to_it_at_all");
        assert_eq!(
            soft.replace(ZWSP, ""),
            "a_very__long_name::with_markup_and/no/breaks_next_to_it_at_all"
        );
        assert!(!soft.contains(&format!("_{ZWSP}")) && !soft.contains(&format!("{ZWSP}_")));
        let linked = breakable(&format!("go to {URL} now")).unwrap_or_default();
        let dests: Vec<String> = Parser::new_ext(&linked, MARKDOWN)
            .filter_map(|e| match e {
                Event::Start(Tag::Link { dest_url, .. }) => Some(dest_url.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(dests, [URL]);
    }
}
