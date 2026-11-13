//! Small pieces the list and the chat share: tones, tiles, dots, notes.

use std::sync::LazyLock;

use rune::color::hex;
use rune::{Color, Cycle, El, TextStyle, col, el, icon, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use super::icons;
use crate::app::State;
use crate::data::Project;
use crate::labels::tone_index;

/// The desktop's project tones (`project_icon.rs`): slate, blue, violet,
/// rose, amber, emerald, teal, orange.
const TONES_DARK: [&str; 8] = [
    "#94a3b8", "#93c5fd", "#c4b5fd", "#fda4af", "#fcd34d", "#6ee7b7", "#5eead4", "#fdba74",
];
const TONES_LIGHT: [&str; 8] = [
    "#475569", "#2563eb", "#7c3aed", "#be123c", "#a16207", "#047857", "#0f766e", "#c2410c",
];

/// The height of a page header's title block: a chat's title over its
/// subtitle. Both pages use it, so the header buttons sit in the same place.
const HEAD_TITLES_HEIGHT: f32 = 39.0;

pub fn head(key: &'static str) -> El {
    row()
        .key(key)
        .w_full()
        .px(8.0)
        .pt(8.0)
        .pb(6.0)
        .gap(6.0)
        .center_y()
        .window_drag()
}

/// A page header's title block, between its leading and trailing buttons.
pub fn head_titles() -> El {
    col()
        .grow(1.0)
        .min_w(0.0)
        .h(HEAD_TITLES_HEIGHT)
        .justify_center()
}

/// Hides the window; the shortcut brings it back.
pub fn hide_button(k: &Kit) -> El {
    IconButton::new(&icons::CLOSE)
        .variant(IconButtonVariant::GhostMuted)
        .label("Hide")
        .help("Hide (Esc)")
        .on_press(State::hide)
        .build(k)
}

static TONES: LazyLock<[[Color; 8]; 2]> = LazyLock::new(|| {
    [TONES_DARK, TONES_LIGHT]
        .map(|tones| tones.map(|t| hex(t).unwrap_or_else(|| panic!("bad tone {t}"))))
});

/// The status tones the phone app uses: working, input, failed, done.
pub struct Tones {
    pub working: Color,
    pub input: Color,
    pub failed: Color,
    pub done: Color,
}

pub fn tones(k: &Kit) -> Tones {
    Tones {
        working: k.c.text_muted,
        input: k.c.warning.icon,
        failed: k.c.danger.icon,
        done: k.c.success.icon,
    }
}

/// The project monogram tile; chats without a project read "H" (Home).
pub fn tile(k: &Kit, project: Option<&Project>, size: f32) -> El {
    let index = project.map_or(6, |p| {
        tone_index(if p.path.is_empty() { &p.name } else { &p.path })
    });
    let tone = TONES[usize::from(!k.dark)][index];
    let name = project.map_or("Home", |p| p.name.trim());
    let letter = name
        .chars()
        .next()
        .map_or("?".to_owned(), |c| c.to_uppercase().collect());
    el().size(size, size)
        .radius(size * 0.28)
        .fill(tone.alpha(0.18))
        .center()
        .child(
            text(letter)
                .font(TextStyle::new(size * 0.62, 600.0))
                .color(tone),
        )
}

pub fn dot(color: Color) -> El {
    el().size(7.0, 7.0).radius(3.5).fill(color)
}

/// Seconds each of `busy`'s dots stays lit.
const BUSY_STEP: f32 = 0.4;
/// Each dot's opacity over a `busy` loop: lit for its third, dim for the rest.
const BUSY_STOPS: &[(f32, f32)] = &[(0.0, 1.0), (0.333, 1.0), (0.334, 0.3), (1.0, 0.3)];

/// An in-progress mark in a `size`-point square: three dots lit in turn.
/// It steps instead of fading, so while only it moves the window redraws
/// twice a step rather than at rune-kit `Spinner`'s 60 frames a second.
pub fn busy(size: f32, k: &Kit) -> El {
    let side = size / 3.6;
    let dots = (0..3).map(|i| {
        el().key(format!("busy{i}"))
            .size(side, side)
            .radius(side / 2.0)
            .fill(k.c.std_text)
            .a11y_hidden()
            .cycle(Cycle {
                period: 3.0 * BUSY_STEP,
                phase: i as f32 / 3.0,
                stops: BUSY_STOPS,
                still: 0.6,
                fps: 2.0 / BUSY_STEP,
            })
    });
    row()
        .role("progressbar")
        .label("Loading")
        .size(size, size)
        .gap(side * 0.3)
        .center_y()
        .children(dots)
}

pub fn faint(k: &Kit, s: impl Into<String>) -> El {
    text(s)
        .font(k.ty.text_small)
        .color(k.c.text_muted)
        .truncate()
}

/// That the engine isn't running, with how to start it, or nothing while it runs.
pub fn engine_note(s: &State, k: &Kit) -> Option<El> {
    if s.engine_up {
        return None;
    }
    let note = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .padding(10.0)
        .radius(10.0)
        .fill(k.c.warning.bg)
        .child(icon(&icons::ALERT, 14.0).color(k.c.warning.icon))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(crate::client::DOWN)
                        .font(k.ty.ui)
                        .color(k.c.text_base)
                        .truncate(),
                )
                .child(
                    text("Start it with ludolph install")
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted)
                        .truncate(),
                ),
        );
    Some(note)
}
