use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use pulldown_cmark::{Event, Options, Parser};
use rune::color::hex;
use rune::{Color, Direction, El, TextStyle, Tx, col, icon, row, text};
use rune_kit::markdown::{Highlight, MarkdownStyle, Syntax};
use rune_kit::syntax::{self, SyntaxTheme};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, Markdown};

use super::breaks::ZWSP;
use super::icons;
use crate::app::{Existing, FileLinks, State};
use crate::paths::expand_home;

/// The Markdown options Rune's Markdown parses with, so source ranges found
/// here match its own.
pub const MARKDOWN: Options = Options::ENABLE_STRIKETHROUGH
    .union(Options::ENABLE_TABLES)
    .union(Options::ENABLE_TASKLISTS);

const PALETTE: [(Syntax, &str); 10] = [
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
];

static COLORS: LazyLock<[(Syntax, Color); 10]> = LazyLock::new(|| {
    PALETTE.map(|(kind, c)| (kind, hex(c).unwrap_or_else(|| panic!("bad colour {c}"))))
});

pub fn markdown_style(k: &Kit) -> MarkdownStyle {
    let style = MarkdownStyle::from_kit(k)
        .inline_code_bg(Color::TRANSPARENT)
        .inline_code_pad(1.0);
    COLORS
        .iter()
        .fold(style, |style, &(kind, c)| style.syntax(kind, c))
}

pub fn frames(md: Markdown, page: &Existing, k: &Kit, key: &str) -> Markdown {
    let k = k.clone();
    let key = key.to_owned();
    let wrapped = page.wrapped_code.clone();
    let copied = page.copied_key().map(str::to_owned);
    md.code_frame(move |_, frame| {
        let key = format!("{key}#code-{}", frame.index);
        card(
            &k,
            &key,
            frame.language,
            frame.source,
            wrapped.contains(&key),
            copied.as_deref() == Some(&key),
        )
    })
}

fn card(k: &Kit, key: &str, language: Option<&str>, source: &str, wrap: bool, copied: bool) -> El {
    let toggle = key.to_owned();
    let copy_key = key.to_owned();
    let shown = source.strip_suffix('\n').unwrap_or(source);
    let copy = shown.to_owned();
    let header = row()
        .w_full()
        .center_y()
        .gap(8.0)
        .px(16.0)
        .pt(10.0)
        .pb(6.0)
        .unselectable()
        .child(icon(&icons::CODE, 16.0).color(k.c.icon_muted))
        .child(
            text(language_label(language))
                .font(k.ty.small)
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        )
        .child(rune::spacer())
        .child(
            IconButton::new(&icons::WORD_WRAP)
                .variant(if wrap {
                    IconButtonVariant::Ghost
                } else {
                    IconButtonVariant::GhostMuted
                })
                .control_size(ControlSize::Small)
                .label(if wrap {
                    "Disable word wrap"
                } else {
                    "Enable word wrap"
                })
                .on_press(move |s: &mut State| s.toggle_code_wrap(&toggle))
                .build(k),
        )
        .child(
            IconButton::new(if copied { &icons::CHECK } else { &icons::COPY })
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label(if copied { "Copied" } else { "Copy code" })
                .help(if copied { "Copied" } else { "Copy code" })
                .on_press_tx(move |s: &mut State, tx: &mut Tx| s.copy_text(&copy_key, &copy, tx))
                .build(k)
                .help_open(copied),
        );
    let theme = COLORS
        .iter()
        .fold(SyntaxTheme::from_kit(k), |theme, &(kind, color)| {
            theme.with(kind, color)
        });
    let spans = theme.spans(&syntax::highlight(language, shown), 0);
    // Keep wrapping flags and the scroll -> pad -> body path in both states so toggling wrap
    // rebreaks already-shaped lines in place and keeps the body's NodeId (preserving selection).
    let body = text(shown)
        .by_line()
        .wrap_anywhere()
        .wrap_greedy()
        .spans(spans)
        .font(TextStyle {
            size: 13.0,
            line_height: 20.0,
            ..k.ty.mono
        })
        .color(k.c.text_base)
        .selectable(true);
    let content = if wrap {
        col()
            .key("scroll")
            .w_full()
            .child(col().w_full().px(16.0).pb(14.0).child(body.w_full()))
    } else {
        col()
            .key("scroll")
            .w_full()
            .scroll_x()
            .scroll_edge_fade(12.0)
            .child(col().px(16.0).pb(14.0).child(body))
    };
    col()
        .key(key)
        .role("code")
        .dir(Direction::Ltr)
        .w_full()
        .min_w(0.0)
        .radius(18.0)
        .fill(k.c.bg_base.mix(Color::BLACK, 0.4))
        .border(0.5, k.c.border_muted)
        .child(header)
        .child(content)
}

fn language_label(language: Option<&str>) -> &str {
    match language {
        None | Some("text" | "txt" | "plain" | "plaintext") => "Plain text",
        Some("sh" | "bash") => "Bash",
        Some("js" | "javascript") => "JavaScript",
        Some("ts" | "typescript") => "TypeScript",
        Some("py" | "python") => "Python",
        Some("rs" | "rust") => "Rust",
        Some("json") => "JSON",
        Some("html") => "HTML",
        Some("css") => "CSS",
        Some("sql") => "SQL",
        Some("yaml" | "yml") => "YAML",
        Some(language) => language,
    }
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

fn inline_code(src: &str) -> Vec<Range<usize>> {
    if !src.contains('`') {
        return Vec::new();
    }
    Parser::new_ext(src, MARKDOWN)
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Code(_)).then_some(range))
        .collect()
}
