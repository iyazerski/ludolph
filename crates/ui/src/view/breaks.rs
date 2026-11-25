//! Display-only Markdown transforms (soft line breaks, list exits, zero-width word breaks)
//! and source-preserving selection copy so synthetic display syntax never leaks to the clipboard.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};
use rune::{SelectedText, TextSelection};

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

pub fn prepare(src: &str, hard_breaks: bool) -> Cow<'_, str> {
    let mut out = if hard_breaks {
        match line_breaks(src) {
            Some(s) => Cow::Owned(s),
            None => Cow::Borrowed(src),
        }
    } else {
        Cow::Borrowed(src)
    };
    if let Some(s) = crate::markup::skill_links(&out) {
        out = Cow::Owned(s);
    }
    if let Some(s) = breakable(&out) {
        out = Cow::Owned(s);
    }
    out
}

pub fn line_breaks(src: &str) -> Option<String> {
    apply_edits_only(src, &line_break_edits(src))
}

pub fn breakable(src: &str) -> Option<String> {
    apply_edits_only(src, &breakable_edits(src))
}

pub fn copy_selection(original: &str, hard_breaks: bool, sel: &TextSelection) -> Option<String> {
    if !sel.texts().iter().any(SelectedText::is_selected) {
        return None;
    }
    if sel.texts().iter().all(|t| t.is_whole() || t.is_empty()) {
        return Some(original.to_owned());
    }
    let (shown, map) = prepare_mapped(original, hard_breaks);
    if let Some(nodes) = parse_nodes(&shown) {
        let parsed: Vec<ParsedText<'_>> = sel
            .texts()
            .iter()
            .map(|text| ParsedText {
                steps: parse_path(text.path()),
                text,
            })
            .collect();
        let refs: Vec<&ParsedText<'_>> = parsed.iter().collect();
        if let Some(shown_range) = resolve_nodes(&nodes, &refs, 0) {
            let orig_range = map.map_range(shown_range);
            if orig_range.start <= orig_range.end
                && original.is_char_boundary(orig_range.start)
                && original.is_char_boundary(orig_range.end)
            {
                return Some(dedent_slice(original, orig_range));
            }
        }
    }
    Some(sel.text().replace(ZWSP, ""))
}

struct ListScope {
    content_col: usize,
    quote_depth: usize,
    exited: bool,
}

fn line_break_edits(src: &str) -> Vec<(Range<usize>, String)> {
    if !src.contains('\n') {
        return Vec::new();
    }
    let mut edits = Vec::new();
    let mut lists: Vec<ListScope> = Vec::new();
    let mut quote_depth = 0usize;
    let mut inline_depth = 0usize;
    let mut just_ended_image = false;

    for (event, range) in Parser::new_ext(src, MARKDOWN).into_offset_iter() {
        match &event {
            Event::Start(Tag::List(_)) => {
                let line_start = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
                let content_col = item_content_col(&src[line_start..], quote_depth);
                lists.push(ListScope {
                    content_col,
                    quote_depth,
                    exited: false,
                });
            }
            Event::Start(Tag::Item) => {
                if let Some(list) = lists.last_mut() {
                    let line_start = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
                    list.content_col = item_content_col(&src[line_start..], list.quote_depth);
                    list.exited = false;
                }
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote_depth = quote_depth.saturating_sub(1),
            Event::Start(
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. },
            ) => inline_depth += 1,
            Event::End(
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image,
            ) => inline_depth = inline_depth.saturating_sub(1),
            Event::SoftBreak => {
                if let Some(blank) =
                    exit_lists(src, range.end, &mut lists, quote_depth, inline_depth)
                {
                    edits.push((range.start..range.start, blank));
                } else if !just_ended_image {
                    edits.push((range.start..range.start, "  ".to_owned()));
                }
            }
            Event::HardBreak => {
                if let Some(blank) =
                    exit_lists(src, range.end, &mut lists, quote_depth, inline_depth)
                {
                    if src[range.clone()].starts_with('\\') {
                        edits.push((range.start..range.start + 1, blank));
                    } else {
                        edits.push((range.start..range.start, blank));
                    }
                }
            }
            _ => {}
        }
        just_ended_image = matches!(event, Event::End(TagEnd::Image));
    }
    edits
}

/// Inserts a blank line when an unindented line follows a list item, ending the list instead
/// of letting CommonMark attach it as a lazy continuation.
fn exit_lists(
    src: &str,
    next_line_start: usize,
    lists: &mut [ListScope],
    quote_depth: usize,
    inline_depth: usize,
) -> Option<String> {
    if inline_depth > 0 || !lists.iter().any(|l| !l.exited) {
        return None;
    }
    let (next_quotes, next_indent, _) =
        strip_quotes_and_indent(&src[next_line_start..], quote_depth);
    let mut exited_any = false;
    for list in lists.iter_mut() {
        if !list.exited
            && (next_quotes < list.quote_depth
                || (next_quotes == list.quote_depth && next_indent < list.content_col))
        {
            list.exited = true;
            exited_any = true;
        }
    }
    if !exited_any {
        return None;
    }
    let mut blank = String::with_capacity(1 + next_quotes);
    blank.push('\n');
    for _ in 0..next_quotes {
        blank.push('>');
    }
    Some(blank)
}

fn item_content_col(line: &str, max_quotes: usize) -> usize {
    let (_, mut col, rest_idx) = strip_quotes_and_indent(line, max_quotes);
    let bytes = &line.as_bytes()[rest_idx..];
    let marker_bytes = match bytes.first() {
        Some(b'-' | b'*' | b'+') => 1,
        Some(b) if b.is_ascii_digit() => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if matches!(bytes.get(digits), Some(b'.' | b')')) {
                digits + 1
            } else {
                0
            }
        }
        _ => 0,
    };
    col += marker_bytes;
    let mut space_cols = 0usize;
    for &b in &bytes[marker_bytes..] {
        match b {
            b' ' => space_cols += 1,
            b'\t' => space_cols += 4 - ((col + space_cols) % 4),
            _ => break,
        }
    }
    // CommonMark: a marker followed by EOL or >=5 spaces has content column `marker + 1`.
    col + if space_cols == 0 || space_cols > 4 {
        1
    } else {
        space_cols
    }
}

fn strip_quotes_and_indent(line: &str, max_quotes: usize) -> (usize, usize, usize) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut quotes = 0;
    while quotes < max_quotes {
        let mut j = i;
        let mut spaces = 0;
        while j < bytes.len() && bytes[j] == b' ' && spaces < 3 {
            j += 1;
            spaces += 1;
        }
        if j < bytes.len() && bytes[j] == b'>' {
            i = j + 1;
            if i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            quotes += 1;
        } else {
            break;
        }
    }
    let mut col = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => col += 1,
            b'\t' => col += 4 - (col % 4),
            _ => break,
        }
        i += 1;
    }
    (quotes, col, i)
}

fn breakable_edits(src: &str) -> Vec<(Range<usize>, String)> {
    if !src.split_whitespace().any(|w| w.len() > LONG) {
        return Vec::new();
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
    edits.sort_by_key(|(r, _)| r.start);
    edits
}

struct EditStage {
    spans: Vec<(Range<usize>, Range<usize>)>,
}

impl EditStage {
    fn map_pos(&self, pos: usize, is_end: bool) -> usize {
        let mut delta: isize = 0;
        for (orig, out) in &self.spans {
            if pos < out.start {
                return (pos as isize - delta) as usize;
            }
            if pos == out.start {
                return if out.start == out.end && is_end {
                    orig.end
                } else {
                    orig.start
                };
            }
            if pos < out.end {
                return if is_end { orig.end } else { orig.start };
            }
            if pos == out.end {
                return orig.end;
            }
            delta += (out.end - out.start) as isize - (orig.end - orig.start) as isize;
        }
        (pos as isize - delta) as usize
    }
}

struct EditMap {
    stages: Vec<EditStage>,
}

impl EditMap {
    fn map_range(&self, range: Range<usize>) -> Range<usize> {
        let mut start = range.start;
        let mut end = range.end;
        for stage in self.stages.iter().rev() {
            start = stage.map_pos(start, false);
            end = stage.map_pos(end, true);
        }
        start..end
    }
}

fn apply_edits_only(src: &str, edits: &[(Range<usize>, String)]) -> Option<String> {
    if edits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(src.len() + src.len() / 8);
    let mut at = 0;
    for (range, with) in edits {
        out.push_str(&src[at..range.start]);
        out.push_str(with);
        at = range.end;
    }
    out.push_str(&src[at..]);
    Some(out)
}

fn apply_edits_mapped(src: &str, edits: &[(Range<usize>, String)]) -> Option<(String, EditStage)> {
    if edits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(src.len() + src.len() / 8);
    let mut spans = Vec::with_capacity(edits.len());
    let mut at = 0;
    for (range, with) in edits {
        out.push_str(&src[at..range.start]);
        let out_start = out.len();
        out.push_str(with);
        let out_end = out.len();
        spans.push((range.clone(), out_start..out_end));
        at = range.end;
    }
    out.push_str(&src[at..]);
    Some((out, EditStage { spans }))
}

fn prepare_mapped(src: &str, hard_breaks: bool) -> (Cow<'_, str>, EditMap) {
    let mut stages = Vec::new();
    let mut out = if hard_breaks {
        match apply_edits_mapped(src, &line_break_edits(src)) {
            Some((s, stage)) => {
                stages.push(stage);
                Cow::Owned(s)
            }
            None => Cow::Borrowed(src),
        }
    } else {
        Cow::Borrowed(src)
    };
    if let Some((s, stage)) = apply_edits_mapped(&out, &crate::markup::skill_link_edits(&out)) {
        stages.push(stage);
        out = Cow::Owned(s);
    }
    if let Some((s, stage)) = apply_edits_mapped(&out, &breakable_edits(&out)) {
        stages.push(stage);
        out = Cow::Owned(s);
    }
    (out, EditMap { stages })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathStep {
    Block(usize),
    Item(usize),
}

struct Node {
    step: PathStep,
    range: Range<usize>,
    children: Vec<Node>,
}

struct ParseFrame {
    range: Range<usize>,
    children: Vec<Node>,
    kind: FrameKind,
}

enum FrameKind {
    Root,
    List,
    Item { inline: Option<Range<usize>> },
    Quote,
    Leaf { para: bool },
}

fn parse_nodes(shown: &str) -> Option<Vec<Node>> {
    let mut stack = vec![ParseFrame {
        range: 0..shown.len(),
        children: Vec::new(),
        kind: FrameKind::Root,
    }];
    for (event, range) in Parser::new_ext(shown, MARKDOWN).into_offset_iter() {
        match event {
            Event::Start(Tag::Paragraph) => {
                flush_item_inline(stack.last_mut());
                stack.push(ParseFrame {
                    range,
                    children: Vec::new(),
                    kind: FrameKind::Leaf { para: true },
                });
            }
            Event::Start(
                Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::Table(_)
                | Tag::HtmlBlock
                | Tag::MetadataBlock(_)
                | Tag::DefinitionList
                | Tag::FootnoteDefinition(_),
            ) => {
                flush_item_inline(stack.last_mut());
                stack.push(ParseFrame {
                    range,
                    children: Vec::new(),
                    kind: FrameKind::Leaf { para: false },
                });
            }
            Event::Start(Tag::List(_)) => {
                flush_item_inline(stack.last_mut());
                stack.push(ParseFrame {
                    range,
                    children: Vec::new(),
                    kind: FrameKind::List,
                });
            }
            Event::Start(Tag::Item) => {
                stack.push(ParseFrame {
                    range,
                    children: Vec::new(),
                    kind: FrameKind::Item { inline: None },
                });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush_item_inline(stack.last_mut());
                stack.push(ParseFrame {
                    range,
                    children: Vec::new(),
                    kind: FrameKind::Quote,
                });
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Table
                | TagEnd::HtmlBlock
                | TagEnd::MetadataBlock(_)
                | TagEnd::DefinitionList
                | TagEnd::FootnoteDefinition
                | TagEnd::List(_)
                | TagEnd::Item
                | TagEnd::BlockQuote(_),
            ) => {
                flush_item_inline(stack.last_mut());
                if let Some(frame) = stack.pop()
                    && let Some(parent) = stack.last_mut()
                {
                    let step = match frame.kind {
                        FrameKind::Item { .. } => PathStep::Item(parent.children.len()),
                        _ => PathStep::Block(parent.children.len()),
                    };
                    parent.children.push(Node {
                        step,
                        range: frame.range,
                        children: frame.children,
                    });
                }
            }
            Event::Rule => {
                flush_item_inline(stack.last_mut());
                if let Some(parent) = stack.last_mut() {
                    let step = PathStep::Block(parent.children.len());
                    parent.children.push(Node {
                        step,
                        range,
                        children: Vec::new(),
                    });
                }
            }
            // Rune splits paragraphs around standalone images and non-1 ordered list interrupts,
            // which shifts block indices relative to pulldown_cmark's events.
            Event::Start(Tag::Image { .. }) => return None,
            Event::SoftBreak | Event::HardBreak => {
                if matches!(
                    stack.last().map(|f| &f.kind),
                    Some(FrameKind::Leaf { para: true } | FrameKind::Item { .. })
                ) && starts_ordered_marker(&shown[range.end..])
                {
                    return None;
                }
                extend_item_inline(stack.last_mut(), &range);
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineHtml(_)
            | Event::TaskListMarker(_)
            | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. }) => {
                extend_item_inline(stack.last_mut(), &range)
            }
            _ => {}
        }
    }
    stack
        .pop()
        .filter(|f| matches!(f.kind, FrameKind::Root))
        .map(|f| f.children)
}

fn flush_item_inline(frame: Option<&mut ParseFrame>) {
    if let Some(ParseFrame {
        children,
        kind: FrameKind::Item { inline },
        ..
    }) = frame
        && let Some(range) = inline.take()
    {
        let step = PathStep::Block(children.len());
        children.push(Node {
            step,
            range,
            children: Vec::new(),
        });
    }
}

fn extend_item_inline(frame: Option<&mut ParseFrame>, range: &Range<usize>) {
    if let Some(ParseFrame {
        kind: FrameKind::Item { inline },
        ..
    }) = frame
    {
        match inline {
            Some(span) => {
                span.start = span.start.min(range.start);
                span.end = span.end.max(range.end);
            }
            None => *inline = Some(range.clone()),
        }
    }
}

fn starts_ordered_marker(rest: &str) -> bool {
    let trimmed = rest.trim_start_matches(' ');
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return false;
    }
    matches!(
        trimmed.as_bytes().get(digits..digits + 2),
        Some([b'.' | b')', b' ' | b'\t'])
    )
}

struct ParsedText<'a> {
    steps: Vec<PathStep>,
    text: &'a SelectedText,
}

fn parse_path(path: &str) -> Vec<PathStep> {
    path.split('/')
        .filter_map(|seg| {
            let rest = seg.get(1..)?;
            if seg.starts_with('b') {
                rest.parse().ok().map(PathStep::Block)
            } else if seg.starts_with('i') {
                rest.parse().ok().map(PathStep::Item)
            } else {
                None
            }
        })
        .collect()
}

fn resolve_nodes(nodes: &[Node], texts: &[&ParsedText<'_>], depth: usize) -> Option<Range<usize>> {
    let mut first = usize::MAX;
    let mut last = 0usize;
    let mut any = false;

    for t in texts {
        if !t.text.is_selected() {
            continue;
        }
        let step = t.steps.get(depth).copied()?;
        let idx = nodes.iter().position(|n| n.step == step)?;
        first = first.min(idx);
        last = last.max(idx);
        any = true;
    }
    if !any {
        return None;
    }

    if first < last {
        let start = node_edge(&nodes[first], texts, depth, false)?;
        let end = node_edge(&nodes[last], texts, depth, true)?;
        return Some(start..end);
    }

    let node = &nodes[first];
    let in_node: Vec<&ParsedText<'_>> = texts
        .iter()
        .copied()
        .filter(|t| t.steps.get(depth) == Some(&node.step))
        .collect();
    if in_node
        .iter()
        .all(|t| t.text.is_whole() || t.text.is_empty())
    {
        return Some(node.range.clone());
    }

    // Selecting only an item's lead paragraph (excluding nested sub-lists) keeps its marker.
    if matches!(node.step, PathStep::Item(_))
        && let Some(lead) = node.children.first()
    {
        let only_lead = in_node
            .iter()
            .filter(|t| t.text.is_selected())
            .all(|t| t.steps.get(depth + 1) == Some(&lead.step));
        let lead_whole = in_node
            .iter()
            .filter(|t| t.steps.get(depth + 1) == Some(&lead.step))
            .all(|t| t.text.is_whole() || t.text.is_empty());
        if only_lead && lead_whole {
            return Some(node.range.start..lead.range.end);
        }
    }

    resolve_nodes(&node.children, &in_node, depth + 1)
}

fn node_edge(node: &Node, texts: &[&ParsedText<'_>], depth: usize, is_end: bool) -> Option<usize> {
    let in_node: Vec<&ParsedText<'_>> = texts
        .iter()
        .copied()
        .filter(|t| t.steps.get(depth) == Some(&node.step))
        .collect();
    let edge_text = if is_end {
        in_node.iter().rev().find(|t| !t.text.is_empty())?
    } else {
        in_node.iter().find(|t| !t.text.is_empty())?
    };
    let at_edge = if is_end {
        edge_text.text.is_selected() && edge_text.text.range().end == edge_text.text.len()
    } else {
        edge_text.text.is_selected() && edge_text.text.range().start == 0
    };
    if at_edge {
        return Some(if is_end {
            node.range.end
        } else {
            node.range.start
        });
    }
    let sel_text = if is_end {
        in_node.iter().rev().find(|t| t.text.is_selected())?
    } else {
        in_node.iter().find(|t| t.text.is_selected())?
    };
    let child_step = sel_text.steps.get(depth + 1).copied()?;
    let child = node.children.iter().find(|c| c.step == child_step)?;
    node_edge(child, &in_node, depth + 1, is_end)
}

fn dedent_slice(original: &str, range: Range<usize>) -> String {
    let slice = original[range.clone()].trim_end_matches(['\n', '\r']);
    let line_start = original[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &original[line_start..range.start];
    if prefix.is_empty()
        || !slice.contains('\n')
        || !prefix.bytes().all(|b| matches!(b, b' ' | b'\t' | b'>'))
    {
        return slice.to_owned();
    }
    let trimmed_prefix = prefix.trim_end();
    let mut out = String::with_capacity(slice.len());
    for (i, line) in slice.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if let Some(rest) = line.strip_prefix(prefix) {
                out.push_str(rest);
                continue;
            }
            if !trimmed_prefix.is_empty() && line.trim_end() == trimmed_prefix {
                continue;
            }
        }
        out.push_str(line);
    }
    out
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

    #[test]
    fn preserves_soft_breaks_and_exits_lists() {
        let cases: &[(&str, Option<&str>)] = &[
            ("single line", None),
            ("para one\n\npara two", None),
            ("1. First\n2. Second", None),
            ("```\nline 1\nline 2\n```", None),
            ("| a | b |\n| - | - |\n| 1 | 2 |", None),
            ("First line\nSecond line", Some("First line  \nSecond line")),
            (
                "First line\r\nSecond line",
                Some("First line  \r\nSecond line"),
            ),
            (
                "1. First\n2. Second\nAfter list",
                Some("1. First\n2. Second\n\nAfter list"),
            ),
            (
                "1. First\n2. Second\nAfter 1\nAfter 2",
                Some("1. First\n2. Second\n\nAfter 1  \nAfter 2"),
            ),
            (
                "1. First\n   continued\n2. Second",
                Some("1. First  \n   continued\n2. Second"),
            ),
            (
                "1. Outer\n   - Inner\n   Outer line 2\nAfter all",
                Some("1. Outer\n   - Inner\n\n   Outer line 2\n\nAfter all"),
            ),
            (
                "> 1. First\n> 2. Second\n> After list",
                Some("> 1. First\n> 2. Second\n>\n> After list"),
            ),
            ("1. **bold\ncontinued**", Some("1. **bold  \ncontinued**")),
            ("1. a\nb\n2. c\nd", Some("1. a\n\nb\n2. c\n\nd")),
            ("1. First\n  After list", Some("1. First\n\n  After list")),
            (
                "- > Quote\n  Still in item\nOutside",
                Some("- > Quote  \n  Still in item\n\nOutside"),
            ),
            (
                "1. First\n2. Second\\\nAfter list",
                Some("1. First\n2. Second\n\nAfter list"),
            ),
        ];
        for &(src, want) in cases {
            assert_eq!(line_breaks(src).as_deref(), want, "source {src:?}");
        }
    }

    #[test]
    fn dedent_strips_bare_quote_lines_and_keeps_trailing_nbsp() {
        let quote = "> p1\n>\n> p2\u{00A0}\n";
        assert_eq!(dedent_slice(quote, 2..quote.len()), "p1\n\np2\u{00A0}");
    }
}
