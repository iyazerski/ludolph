use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::LazyLock;

use rune::color::hex;
use rune::{
    Color, Cycle, Drag, El, Live, Phase, ScrollPos, ScrollSpec, Span, Spring, SpringPair,
    TextStyle, Transition, Tx, col, el, icon, row, text, tok,
};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use super::icons;
use crate::app::State;
use crate::data::Project;
use crate::labels::tone_index;
use crate::markup::Shade;

const TONES: [&str; 8] = [
    "#94a3b8", "#93c5fd", "#c4b5fd", "#fda4af", "#fcd34d", "#6ee7b7", "#5eead4", "#fdba74",
];

const HEAD_TITLES_HEIGHT: f32 = 39.0;

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
pub fn page(el: El, key: impl Into<String>) -> El {
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

pub fn markdown_spans(k: &Kit, shades: Vec<(Range<usize>, Shade)>) -> Vec<Span> {
    shades
        .into_iter()
        .map(|(range, shade)| {
            let span = Span::new(range.start, range.end);
            match shade {
                Shade::Heading | Shade::Strong => span.weight(600.0),
                Shade::Marker | Shade::Target => span.color(k.c.text_faint),
                Shade::Link => span.color(k.c.text_accent),
                Shade::WebUrl => span.color(k.c.text_accent).underline(k.c.text_accent),
                Shade::Code => span.mono().color(super::theme::inline_code(k)),
                Shade::Emphasis => span.italic(),
                Shade::Skill => span.color(k.c.text_accent).weight(600.0),
            }
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

const SCROLL_TRACK_W: f32 = 12.0;
const SCROLL_THUMB_W: f32 = 5.0;
const SCROLL_INSET: f32 = 4.0;
const SCROLL_MIN_THUMB: f32 = 24.0;
const KEPT_SCROLL_BARS: usize = 64;

struct VisibleBar {
    pos: Cell<ScrollPos>,
    hover: Cell<bool>,
    dragging: Cell<bool>,
    rest: Cell<Color>,
    active: Cell<Color>,
    thumb: Live<[f32; 4]>,
    fill: Live<Color>,
}

impl VisibleBar {
    fn new(rest: Color, active: Color) -> Self {
        Self {
            pos: Cell::new(ScrollPos::default()),
            hover: Cell::new(false),
            dragging: Cell::new(false),
            rest: Cell::new(rest),
            active: Cell::new(active),
            thumb: Live::new([0.0; 4]),
            fill: Live::new(rest),
        }
    }

    fn restyle(&self, rest: Color, active: Color) {
        self.rest.set(rest);
        self.active.set(active);
        self.update_fill();
    }

    fn update_fill(&self) {
        let color = if self.hover.get() || self.dragging.get() {
            self.active.get()
        } else {
            self.rest.get()
        };
        self.fill.set(color);
    }

    fn thumb_metrics(at: ScrollPos) -> Option<(f32, f32, f32)> {
        let shown = at.drawn[1];
        let max = at.max[1];
        let track_len = shown - 2.0 * SCROLL_INSET;
        if max <= 0.5 || track_len < SCROLL_MIN_THUMB {
            return None;
        }
        let len = (track_len * shown / (shown + max)).clamp(SCROLL_MIN_THUMB, track_len);
        let travel = (track_len - len).max(0.0);
        let top = SCROLL_INSET + travel * (at.offset[1] / max).clamp(0.0, 1.0);
        Some((top, len, travel))
    }

    fn follow(&self, at: ScrollPos) {
        self.pos.set(at);
        let rect = match Self::thumb_metrics(at) {
            Some((top, len, _)) => [
                (SCROLL_TRACK_W - SCROLL_THUMB_W) / 2.0,
                top,
                SCROLL_THUMB_W,
                len,
            ],
            None => [0.0; 4],
        };
        self.thumb.snap(rect);
    }

    fn set_hover(&self, inside: bool) {
        if self.hover.replace(inside) != inside {
            self.update_fill();
        }
    }

    fn drag(&self, target: &str, d: &mut Drag, tx: &mut Tx) {
        let at = self.pos.get();
        let Some((top, len, travel)) = Self::thumb_metrics(at) else {
            return;
        };
        match d.phase {
            Phase::Down => {
                self.dragging.set(true);
                self.update_fill();
                let on_thumb = (top..=top + len).contains(&d.y);
                if on_thumb {
                    d.data = d.y - top;
                } else {
                    d.data = len / 2.0;
                    if travel > 0.0 {
                        let y =
                            ((d.y - d.data - SCROLL_INSET) / travel).clamp(0.0, 1.0) * at.max[1];
                        tx.scroll_to_instant(target, 0.0, y);
                    }
                }
            }
            Phase::Move => {
                if travel > 0.0 {
                    let y = ((d.y - d.data - SCROLL_INSET) / travel).clamp(0.0, 1.0) * at.max[1];
                    tx.scroll_to_instant(target, 0.0, y);
                }
            }
            Phase::Up => {
                self.dragging.set(false);
                self.update_fill();
            }
        }
    }
}

thread_local! {
    static VISIBLE_BARS: RefCell<HashMap<(u64, String), Rc<VisibleBar>>> =
        RefCell::new(HashMap::new());
}

fn kept_visible_bar(key: &str, rest: Color, active: Color) -> Rc<VisibleBar> {
    let id = (rune::current_runtime_id(), key.to_owned());
    VISIBLE_BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        if bars.len() >= KEPT_SCROLL_BARS && !bars.contains_key(&id) {
            bars.clear();
        }
        let bar = bars
            .entry(id)
            .or_insert_with(|| Rc::new(VisibleBar::new(rest, active)))
            .clone();
        bar.restyle(rest, active);
        bar
    })
}

/// Vertical scroll container with an always-visible scrollbar in a right gutter, driven through
/// live values so scrolling does not rebuild the view.
pub fn visible_scroll_bar(k: &Kit, scroll_key: String, max_h: f32, content: El) -> El {
    let bar = kept_visible_bar(&scroll_key, k.c.border_strong, k.c.text_muted);
    let follow = bar.clone();
    let hover = bar.clone();
    let drag = bar.clone();
    let target = scroll_key.clone();
    let viewport = content
        .key(scroll_key.clone())
        .grow(1.0)
        .min_w(0.0)
        .max_h(max_h)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .chain_scroll_offset(move |at| follow.follow(at));
    let track = el()
        .key(format!("{scroll_key}-scrollbar"))
        .w(SCROLL_TRACK_W)
        .h(max_h)
        .wheel_scrolls(scroll_key.clone())
        .a11y_hidden()
        .on_hover_change_any(move |inside, tx| {
            tx.live_only();
            hover.set_hover(inside);
        })
        .on_drag_any(move |d, tx| {
            tx.live_only();
            drag.drag(&target, d, tx);
        })
        .child(
            el().key(format!("{scroll_key}-thumb"))
                .absolute(0.0, 0.0)
                .live_rect(&bar.thumb)
                .radius(SCROLL_THUMB_W / 2.0)
                .live_fill(&bar.fill)
                .a11y_hidden(),
        );
    row().w_full().child(viewport).child(track)
}
