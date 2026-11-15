use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use pulldown_cmark::{Event, Options, Parser};
use rune::Color;
use rune::color::hex;
use rune_kit::Kit;
use rune_kit::markdown::{Highlight, MarkdownStyle, Syntax};

use super::breaks::ZWSP;
use crate::app::FileLinks;
use crate::paths::expand_home;

/// The Markdown options Rune's Markdown parses with, so source ranges found
/// here match its own.
pub const MARKDOWN: Options = Options::ENABLE_STRIKETHROUGH
    .union(Options::ENABLE_TABLES)
    .union(Options::ENABLE_TASKLISTS);

const PALETTE: [[(Syntax, &str); 10]; 2] = [
    [
        (Syntax::Keyword, "#c678dd"),
        (Syntax::String, "#98c379"),
        (Syntax::Regex, "#98c379"),
        (Syntax::Number, "#d19a66"),
        (Syntax::Comment, "#7f848e"),
        (Syntax::Type, "#e5c07b"),
        (Syntax::Function, "#61afef"),
        (Syntax::Heading, "#61afef"),
        (Syntax::Property, "#e06c75"),
        (Syntax::Attribute, "#e06c75"),
    ],
    [
        (Syntax::Keyword, "#a626a4"),
        (Syntax::String, "#50a14f"),
        (Syntax::Regex, "#50a14f"),
        (Syntax::Number, "#986801"),
        (Syntax::Comment, "#a0a1a7"),
        (Syntax::Type, "#c18401"),
        (Syntax::Function, "#4078f2"),
        (Syntax::Heading, "#4078f2"),
        (Syntax::Property, "#e45649"),
        (Syntax::Attribute, "#e45649"),
    ],
];

static COLORS: LazyLock<[Vec<(Syntax, Color)>; 2]> = LazyLock::new(|| {
    PALETTE.map(|tones| {
        tones
            .iter()
            .map(|&(kind, c)| (kind, hex(c).unwrap_or_else(|| panic!("bad colour {c}"))))
            .collect()
    })
});

pub fn markdown_style(k: &Kit) -> MarkdownStyle {
    let style = MarkdownStyle::from_kit(k)
        .inline_code_bg(Color::TRANSPARENT)
        .inline_code_pad(1.0);
    palette(k)
        .iter()
        .fold(style, |style, &(kind, c)| style.syntax(kind, c))
}

pub fn inline_code_marks(
    k: &Kit,
    src: &str,
    link: impl Fn(&str) -> Option<String>,
) -> Vec<(Range<usize>, Highlight)> {
    let color = super::theme::inline_code(k);
    inline_code(src)
        .into_iter()
        .filter(|r| link(src[r.clone()].trim_matches('`').trim()).is_none())
        .map(|r| (r, Highlight::new().bg(Color::TRANSPARENT).fg(color)))
        .collect()
}

pub fn file_links(
    cwd: &str,
    found: &FileLinks,
) -> impl Fn(&str) -> Option<String> + Clone + 'static {
    let cwd = expand_home(cwd);
    let found = found.clone();
    move |code| {
        found
            .borrow_mut()
            .entry(code.to_owned())
            .or_insert_with(|| file_link(code, &cwd))
            .clone()
    }
}

fn file_link(code: &str, cwd: &Path) -> Option<String> {
    let code = code.replace(ZWSP, "");
    let mut path = code.as_str();
    for _ in 0..2 {
        if let Some((head, n)) = path.rsplit_once(':')
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            path = head;
        }
    }
    // Skip code that can't be a path before asking the disk.
    let pathlike = (path.contains('/') || path.contains('.'))
        && path.contains(char::is_alphanumeric)
        && !path.contains(char::is_whitespace)
        && !path.contains("://");
    if !pathlike {
        return None;
    }
    let full = cwd.join(expand_home(path));
    full.exists().then(|| full.to_string_lossy().into_owned())
}

fn palette(k: &Kit) -> &'static [(Syntax, Color)] {
    &COLORS[usize::from(!k.dark)]
}

fn inline_code(src: &str) -> Vec<Range<usize>> {
    if !src.contains('`') {
        return Vec::new();
    }
    Parser::new_ext(src, MARKDOWN)
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Code(_)).then_some(range))
        .collect()
}
