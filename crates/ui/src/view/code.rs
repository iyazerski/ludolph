use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use rune::color::hex;
use rune::{Color, Direction, El, Link, TextStyle, Tx, col, icon, rich, row, text};
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

pub fn user_message(page: &Existing, k: &Kit, source: &str, key: &str) -> El {
    let mut out = col().gap(10.0).min_w(0.0);
    let mut end = 0;
    let mut index = 0;
    let mut events = Parser::new_ext(source, MARKDOWN).into_offset_iter();
    while let Some((event, range)) = events.next() {
        let Event::Start(Tag::CodeBlock(kind)) = event else {
            continue;
        };
        if range.start > end {
            out = out.child(user_prose(k, &source[end..range.start]));
        }
        let mut code = String::new();
        for (event, _) in events.by_ref() {
            match event {
                Event::Text(part) => code.push_str(&part),
                Event::End(TagEnd::CodeBlock) => break,
                _ => {}
            }
        }
        let language = match &kind {
            CodeBlockKind::Fenced(info) => info.split_whitespace().next(),
            CodeBlockKind::Indented => None,
        };
        let key = format!("{key}#code-{index}");
        out = out.child(card(
            k,
            &key,
            language,
            &code,
            page.wrapped_code.contains(&key),
            page.just_copied(&key),
        ));
        end = range.end;
        index += 1;
    }
    if end < source.len() {
        out = out.child(user_prose(k, &source[end..]));
    }
    let mut bubble = col()
        .shrink()
        .min_w(0.0)
        .px(12.0)
        .py(8.0)
        .radius(14.0)
        .fill(super::theme::message_bg(k));
    if index > 0 {
        bubble = bubble.w_full();
    }
    bubble.child(out)
}

fn user_prose(k: &Kit, source: &str) -> El {
    let shown = source.trim().to_owned();
    let spans = super::widgets::markdown_spans(k, &shown);
    let links = crate::markup::links(&shown);
    let mut body = rich(shown, spans)
        .font(k.ty.body.lh(22.4))
        .color(k.c.text_base)
        .wrap_anywhere()
        .selectable(true);
    for (range, url) in links {
        body = body.link(Link::new(range, url));
    }
    body.on_link(|s: &mut State, url: &str, tx: &mut Tx| s.open_link(url, tx))
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
                .on_press_tx(move |s: &mut State, tx: &mut Tx| s.copy_text(&copy_key, &copy, tx))
                .build(k),
        );
    let theme = palette(k)
        .iter()
        .fold(SyntaxTheme::from_kit(k), |theme, &(kind, color)| {
            theme.with(kind, color)
        });
    let spans = theme.spans(&syntax::highlight(language, shown), 0);
    let mut body = text(shown)
        .spans(spans)
        .font(TextStyle {
            size: 13.0,
            line_height: 20.0,
            ..k.ty.mono
        })
        .color(k.c.text_base)
        .selectable(true);
    let content = if wrap {
        body = body.wrap_anywhere().w_full();
        col().w_full().px(16.0).pb(14.0).child(body)
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
        .fill(k.c.bg_base.mix(k.c.text_base, 0.06))
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
