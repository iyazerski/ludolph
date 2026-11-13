//! How code reads in the transcript: code blocks in an editor's syntax
//! colours (One Dark and One Light), and inline code as coloured monospace
//! text without a chip. The phone app (`relay/assets/app.css`) uses the same
//! colours.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use pulldown_cmark::{Event, Options, Parser};
use rune::Color;
use rune::color::hex;
use rune_kit::Kit;
use rune_kit::markdown::{Highlight, MarkdownStyle, Syntax};

use super::breaks::ZWSP;
use crate::app::FileLinks;

/// The Markdown options Rune's Markdown parses with, so source ranges found
/// here match its own.
pub const MARKDOWN: Options = Options::ENABLE_STRIKETHROUGH
    .union(Options::ENABLE_TABLES)
    .union(Options::ENABLE_TASKLISTS);

/// Token colours, dark then light. Inline code takes the keyword colour.
/// Regexes, headings and attributes share a colour as the phone app's
/// highlight.js classes do; other kinds keep the kit's colours.
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

/// The transcript's Markdown look: the kit's, with these code colours and no
/// chip behind inline code.
pub fn markdown_style(k: &Kit) -> MarkdownStyle {
    let style = MarkdownStyle::from_kit(k)
        .inline_code_bg(Color::TRANSPARENT)
        .inline_code_pad(1.0);
    palette(k)
        .iter()
        .fold(style, |style, &(kind, c)| style.syntax(kind, c))
}

/// Colour the inline code of Markdown `src`, as [`rune_kit::Markdown::highlights`] takes it.
/// Code that `link` makes a link keeps the link's colour.
pub fn inline_code_marks(
    k: &Kit,
    src: &str,
    link: impl Fn(&str) -> Option<String>,
) -> Vec<(Range<usize>, Highlight)> {
    let color = palette(k)[0].1;
    inline_code(src)
        .into_iter()
        .filter(|r| link(src[r.clone()].trim_matches('`').trim()).is_none())
        .map(|r| (r, Highlight::new().bg(Color::TRANSPARENT).fg(color)))
        .collect()
}

/// Link inline code that names a file or folder in `cwd` to its path, as the
/// desktop links paths that exist. `found` keeps each answer.
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

/// The absolute path of the file or folder `code` names in `cwd`, if it exists.
/// `src/main.rs:42:7` names `src/main.rs`.
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

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(
            || PathBuf::from(path),
            |home| PathBuf::from(home).join(rest),
        ),
        None => PathBuf::from(path),
    }
}

fn palette(k: &Kit) -> &'static [(Syntax, Color)] {
    &COLORS[usize::from(!k.dark)]
}

/// The source byte ranges of `src`'s inline code spans, backticks included.
fn inline_code(src: &str) -> Vec<Range<usize>> {
    if !src.contains('`') {
        return Vec::new();
    }
    Parser::new_ext(src, MARKDOWN)
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Code(_)).then_some(range))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_code_only() {
        let cases: &[(&str, &[&str])] = &[
            ("no code here", &[]),
            ("run `cargo test` then `make`", &["`cargo test`", "`make`"]),
            ("a ``tick ` inside`` b", &["``tick ` inside``"]),
            ("```rust\nlet x = `a`;\n```\n", &[]),
            (
                "- item `x`\n\n| a | `b` |\n|---|---|\n| `c` | d |",
                &["`x`", "`b`", "`c`"],
            ),
            ("an `unclosed tick", &[]),
        ];
        for (src, want) in cases {
            let got: Vec<&str> = inline_code(src).into_iter().map(|r| &src[r]).collect();
            assert_eq!(&got, want, "source {src:?}");
        }
    }

    #[test]
    fn links_code_that_names_an_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        let main = dir.path().join("src/main.rs").display().to_string();
        let cases: &[(&str, Option<&str>)] = &[
            ("src/main.rs", Some(&main)),
            ("src/main.rs:42:7", Some(&main)),
            ("src/\u{200B}main.rs", Some(&main)),
            (&main, Some(&main)),
            ("src/lib.rs", None),
            ("cargo test", None),
            ("main", None),
            ("https://x.dev/src", None),
        ];
        for (code, want) in cases {
            assert_eq!(
                file_link(code, dir.path()).as_deref(),
                *want,
                "code {code:?}"
            );
        }
    }
}
