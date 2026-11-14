//! The phone app's icons (`relay/assets/app.js` `ICONS`), with each group's
//! attributes moved onto its elements.

use std::sync::LazyLock;

use rune::Icon;
/// A step of thinking among tool calls. The phone leaves thinking out.
pub use rune_theme_opencode::icons::OUTLINE_BRAIN as THOUGHT;
/// The settings. The phone keeps its settings under its menu, without one.
pub use rune_theme_opencode::icons::SETTINGS_GEAR as SETTINGS;
/// A new chat, and a project's folder in the main window's sidebar. The
/// phone has neither.
pub use rune_theme_opencode::icons::{EDIT as NEW_CHAT, FOLDER};
/// Expanding the quick window's chat into the main window, and popping the
/// main window's chat out into the quick one. The phone has neither.
pub use rune_theme_opencode::icons::{OUTLINE_EXPAND as EXPAND, SQUARE_ARROW_TOP_RIGHT as POP_OUT};

const fn icon(name: &'static str, view: f32, body: &'static str) -> Icon {
    Icon { name, view, body }
}

/// A project's folder while its chats show, drawn to match the theme's
/// closed `FOLDER`.
pub static FOLDER_OPEN: Icon = icon(
    "folder-open",
    16.0,
    r#"<path d="M1.33337 13.3333V2H6.66671L8.66671 4H13.3334V6.66667M1.33337 13.3333L3.33337 6.66667H15.3334L13.3334 13.3333H1.33337Z" stroke="currentColor" stroke-miterlimit="10" stroke-linecap="square" stroke-linejoin="round" fill="none"/>"#,
);

pub static BACK: Icon = icon(
    "back",
    24.0,
    r#"<path d="M14.75 17.5 9.25 12 14.75 6.5" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"/>"#,
);
pub static COPY: Icon = icon(
    "copy",
    24.0,
    r#"<g fill="none" stroke="currentColor" stroke-width="1.5"><path d="M6.6 11.4c0-2.263 0-3.394.703-4.097C8.006 6.6 9.137 6.6 11.4 6.6h4.8c2.263 0 3.394 0 4.097.703c.703.703.703 1.834.703 4.097v4.8c0 2.263 0 3.394-.703 4.097c-.703.703-1.834.703-4.097.703h-4.8c-2.263 0-3.394 0-4.097-.703C6.6 19.594 6.6 18.463 6.6 16.2z"/><path d="M17.396 6.6c-.005-1.774-.05-2.707-.657-3.314C16.036 2.583 14.905 2.583 12.642 2.583h-3.6c-2.263 0-3.394 0-4.097.703c-.703.703-.703 1.834-.703 4.097v3.6c0 2.263 0 3.394.703 4.097c.433.433.996.587 1.655.635"/></g>"#,
);
pub static CHEV_DOWN: Icon = icon(
    "chev-down",
    24.0,
    r#"<path d="M17.5 9.25 12 14.75 6.5 9.25" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"/>"#,
);
pub static DIFF: Icon = icon(
    "diff",
    24.0,
    r#"<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7.5z"/><path d="M14 3v5h5M12 10.5v5M9.5 13h5M9.5 18h5"/></g>"#,
);
pub static PLUS: Icon = icon(
    "plus",
    24.0,
    r#"<path d="M12 5v14M5 12h14" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"/>"#,
);
pub static MORE: Icon = icon(
    "more",
    24.0,
    r#"<circle cx="3.75" cy="12" r="1.6" fill="currentColor"/><circle cx="12" cy="12" r="1.6" fill="currentColor"/><circle cx="20.25" cy="12" r="1.6" fill="currentColor"/>"#,
);
pub static SEND: Icon = icon(
    "send",
    24.0,
    r#"<path d="M12 20V4m0 0l6 6m-6-6l-6 6" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.75"/>"#,
);
pub static STOP: Icon = icon(
    "stop",
    10.0,
    r#"<rect x="1" y="1" width="8" height="8" rx="2" fill="currentColor"/>"#,
);
pub static SEARCH: Icon = icon(
    "search",
    24.0,
    r#"<circle cx="11.5" cy="11.5" r="9.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M18.5 18.5L22 22" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
);
pub static CHECK: Icon = icon(
    "check",
    16.0,
    r#"<path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
);
pub static TERMINAL: Icon = icon(
    "terminal",
    16.0,
    r#"<rect x="1.75" y="2.25" width="12.5" height="11.5" rx="3.25" fill="none" stroke="currentColor" stroke-width="1.25"/><path d="M4.75 6.1 6.65 8l-1.9 1.9" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"/><path d="M8.4 10.4h2.85" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/>"#,
);
pub static READ: Icon = icon(
    "read",
    24.0,
    r#"<rect x="3" y="2" width="18" height="20" rx="5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 10h8m-8 4h5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
);
pub static EDIT: Icon = icon(
    "edit",
    24.0,
    r#"<path d="M14.36 4.08l.93-.93a3.93 3.93 0 0 1 5.56 5.56l-.93.93M14.36 4.08l-8.52 8.52c-.58.58-.87.87-1.11 1.19a6.6 6.6 0 0 0-.75 1.21c-.17.36-.3.75-.56 1.53l-1.1 3.28 3.28-1.1c.78-.26 1.17-.39 1.53-.56a6.6 6.6 0 0 0 1.21-.75c.32-.25.61-.53 1.19-1.11l8.52-8.52" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"#,
);
pub static FETCH: Icon = icon(
    "fetch",
    24.0,
    r#"<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="1.5"/><ellipse cx="12" cy="12" rx="4" ry="10" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M2 12h20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
);
pub static TODO: Icon = icon(
    "todo",
    24.0,
    r#"<path d="M2 5.5L3.214 7L7.5 3M2 12.5L3.214 14L7.5 10M2 19.5L3.214 21L7.5 17" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M22 19H12m10-7H12m10-7H12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
);
pub static TOOL: Icon = icon(
    "tool",
    24.0,
    r#"<rect x="2.5" y="2.5" width="8" height="8" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="13.5" y="2.5" width="8" height="8" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="2.5" y="13.5" width="8" height="8" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="13.5" y="13.5" width="8" height="8" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/>"#,
);
pub static BOT: Icon = icon(
    "bot",
    24.0,
    r#"<rect x="4.75" y="7.75" width="14.5" height="11.5" rx="2.75" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 7.5V5.25M9.25 12.25V14M14.75 12.25V14M2 12.5V15M22 12.5V15" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="12" cy="3.75" r="1.25" fill="currentColor"/>"#,
);
pub static ARCHIVE: Icon = icon(
    "archive",
    24.0,
    r#"<path d="M2 12c0-4.714 0-7.071 1.464-8.536C4.93 2 7.286 2 12 2s7.071 0 8.535 1.464C22 4.93 22 7.286 22 12" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="2" y="6" width="20" height="16" rx="5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="m9.5 14.4l1.429 1.6l3.571-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#,
);
pub static SKILL: Icon = icon(
    "skill",
    24.0,
    r#"<path d="M3.84 7.92a2.88 2.88 0 0 1 4.08-4.08l12.23 12.23a2.88 2.88 0 0 1-4.08 4.08Z" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M6 10L10 6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
);
pub static ALERT: Icon = icon(
    "alert",
    24.0,
    r#"<path d="M5.312 10.762C8.23 5.587 9.689 3 12 3s3.77 2.587 6.688 7.762l.364.644c2.425 4.3 3.638 6.45 2.542 8.022S17.786 21 12.364 21h-.728c-5.422 0-8.134 0-9.23-1.572s.117-3.722 2.542-8.022z" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 8v5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="12" cy="16" r="1" fill="currentColor"/>"#,
);
pub static BRANCH: Icon = icon(
    "branch",
    24.0,
    r#"<path d="M6 8.5v7M6 12h10a2 2 0 0 0 2-2V8.5" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"/><circle cx="6" cy="6.25" r="2.25" fill="none" stroke="currentColor" stroke-width="1.75"/><circle cx="6" cy="17.75" r="2.25" fill="none" stroke="currentColor" stroke-width="1.75"/><circle cx="18" cy="6.25" r="2.25" fill="none" stroke="currentColor" stroke-width="1.75"/>"#,
);

pub static MIC: Icon = icon(
    "mic",
    24.0,
    r#"<rect x="8.75" y="2.75" width="6.5" height="11.5" rx="3.25" fill="none" stroke="currentColor" stroke-width="1.75"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5v3.75" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round"/>"#,
);
pub static CLOSE: Icon = icon(
    "close",
    24.0,
    r#"<path d="M17 7 7 17M7 7l10 10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
);
/// The track under a context ring.
pub static RING: Icon = icon(
    "ring",
    16.0,
    r#"<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2"/>"#,
);

/// The used share of a context ring, one per percent: icons are static SVG.
static ARCS: LazyLock<Vec<Icon>> = LazyLock::new(|| (0..=100).map(arc).collect());

fn arc(percent: u32) -> Icon {
    let turn = f64::from(percent) / 100.0 * std::f64::consts::TAU;
    let (x, y) = (8.0 + 6.0 * turn.sin(), 8.0 - 6.0 * turn.cos());
    let large = u8::from(percent > 50);
    let shape = match percent {
        0 => String::new(),
        100 => {
            r#"<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2"/>"#
                .to_owned()
        }
        _ => format!(
            r#"<path d="M8 2A6 6 0 {large} 1 {x:.3} {y:.3}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#
        ),
    };
    // Built once per percent for the life of the app.
    icon(
        Box::leak(format!("ring-{percent}").into_boxed_str()),
        16.0,
        Box::leak(shape.into_boxed_str()),
    )
}

/// The arc of a context ring `fraction` full.
pub fn ring(fraction: f64) -> &'static Icon {
    let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as usize;
    &ARCS[percent]
}

/// A model's provider mark, from its `provider/model` id, else a bot.
pub fn mark(model: &str) -> &'static Icon {
    let (provider, name) = model.split_once('/').unwrap_or((model, model));
    // A gateway like bifrost has no mark, so the model's family stands in.
    let family = [
        ("claude", "anthropic"),
        ("gpt", "openai"),
        ("gemini", "google"),
        ("grok", "xai"),
    ]
    .into_iter()
    .find_map(|(prefix, maker)| name.starts_with(prefix).then_some(maker));
    let named = rune_theme_opencode::provider_icons::named;
    named(provider)
        .or_else(|| family.and_then(named))
        .unwrap_or(&BOT)
}

/// The icon of one of pi's tools, as the phone shows it.
pub fn tool(name: &str) -> &'static Icon {
    match name {
        "bash" => &TERMINAL,
        "read" | "ls" => &READ,
        "write" | "edit" => &EDIT,
        "grep" | "find" => &SEARCH,
        _ if name.contains("fetch") || name.contains("web") => &FETCH,
        _ if name.contains("todo") => &TODO,
        _ => &TOOL,
    }
}
