use std::sync::LazyLock;

use rune::color::hex;
use rune::{
    Color, Cycle, El, Span, Spring, SpringPair, TextStyle, Transition, col, el, icon, row, text,
    tok,
};
use rune_kit::{ComboboxOption, IconButton, IconButtonVariant, Kit};

use super::icons;
use crate::app::State;
use crate::data::{Model, Project};
use crate::labels::{short_model, tone_index};
use crate::markup::{self, Shade};

const TONES: [&str; 8] = [
    "#94a3b8", "#93c5fd", "#c4b5fd", "#fda4af", "#fcd34d", "#6ee7b7", "#5eead4", "#fdba74",
];

const HEAD_TITLES_HEIGHT: f32 = 39.0;

/// How far content rises as it comes in, in points.
const RISE: f32 = 12.0;

/// Rises into place and fades in, then only fades out, so leaving content doesn't drift.
pub fn rise() -> Transition {
    Transition::asymmetric(
        Transition::offset(0.0, RISE).combined(Transition::opacity()),
        Transition::opacity(),
    )
}

/// Fades in but leaves at once, so a page and the one it replaces don't show through each other.
pub fn swap() -> Transition {
    Transition::opacity().springs(SpringPair::new(tok::motion::SETTLE, Spring::NONE))
}

/// Keys a whole page so Rune swaps it rather than springing its unkeyed parts from the last
/// page's layout.
pub fn page(el: El, key: &'static str) -> El {
    el.key(key).transition(swap())
}

/// Hides `el` until its first layout, then rises it in, for content that mounts out of sight
/// in the commit that scrolls to it: Rune lands such an entrance before the scroll brings it
/// in. `show` marks it shown and returns whether it wasn't already.
pub fn rise_into_view(el: El, shown: bool, show: impl Fn(&mut State) -> bool + 'static) -> El {
    if shown {
        return el;
    }
    el.opacity(0.0)
        .offset(0.0, RISE)
        .on_layout_tx(move |s: &mut State, _, tx| {
            if show(s) {
                // A layout report isn't input, so Rune would land the change at once.
                tx.animated();
            } else {
                tx.live_only();
            }
        })
}

pub fn markdown_spans(k: &Kit, text: &str) -> Vec<Span> {
    markup::shades(text)
        .into_iter()
        .map(|(range, shade)| {
            let span = Span::new(range.start, range.end);
            match shade {
                Shade::Heading | Shade::Strong => span.weight(600.0),
                Shade::Marker | Shade::Target => span.color(k.c.text_faint),
                Shade::Link => span.color(k.c.text_accent),
                Shade::Code => span.mono().color(super::theme::inline_code(k)),
                Shade::Emphasis => span.italic(),
                Shade::Skill => span.color(k.c.text_accent).weight(600.0),
            }
        })
        .collect()
}

pub fn model_options<'a>(models: impl IntoIterator<Item = &'a Model>) -> Vec<ComboboxOption> {
    models
        .into_iter()
        .map(|model| {
            let label = model
                .label
                .clone()
                .unwrap_or_else(|| short_model(&model.id).to_owned());
            let mut option = ComboboxOption::new(model.id.clone(), label);
            option.detail = model
                .id
                .split_once('/')
                .map_or("", |(provider, _)| provider)
                .to_owned();
            option
        })
        .collect()
}

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

pub fn head_titles() -> El {
    col()
        .grow(1.0)
        .min_w(0.0)
        .h(HEAD_TITLES_HEIGHT)
        .justify_center()
}

pub fn hide_button(k: &Kit) -> El {
    IconButton::new(&icons::CLOSE)
        .variant(IconButtonVariant::GhostMuted)
        .label("Hide")
        .help("Hide (Esc)")
        .on_press(State::hide)
        .build(k)
}

static TONE_COLORS: LazyLock<[Color; 8]> =
    LazyLock::new(|| TONES.map(|t| hex(t).unwrap_or_else(|| panic!("bad tone {t}"))));

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

pub fn tile(project: Option<&Project>, size: f32) -> El {
    let index = project.map_or(6, |p| {
        tone_index(if p.path.is_empty() { &p.name } else { &p.path })
    });
    let tone = TONE_COLORS[index];
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

const BUSY_STEP: f32 = 0.4;
const BUSY_STOPS: &[(f32, f32)] = &[(0.0, 1.0), (0.333, 1.0), (0.334, 0.3), (1.0, 0.3)];

/// Stepped dots avoid the kit spinner's 60 Hz redraw loop.
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
                    text("Its log is in ~/Library/Logs/Ludolph")
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted)
                        .truncate(),
                ),
        );
    Some(note)
}
