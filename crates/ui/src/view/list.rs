//! The chat list, as the phone shows it: chats grouped by project, most
//! recently active first, with a search over titles, projects and previews
//! and a filter by status.

use chrono::{DateTime, TimeDelta, Utc};
use rune::{El, TextStyle, Tx, col, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, IconButton, IconButtonVariant, Kit, SegmentedControl, TextInput,
};

use super::icons;
use super::widgets::{busy, dot, engine_note, faint, head, head_titles, hide_button, tile, tones};
use crate::app::{GROUP_CHATS, ListFilter, State};
use crate::data::{Chat, Status};
use crate::labels::{ago, short_model};

/// The search field's height; its ends are fully round, as on the phone.
const SEARCH_HEIGHT: f32 = 32.0;
/// How long ago a chat may have been active to count as recent.
const RECENT: TimeDelta = TimeDelta::minutes(15);

pub fn view(s: &State, k: &Kit) -> El {
    let header = head("list-head")
        .child(
            // The inset keeps the title in line with the search field below.
            head_titles().pl(4.0).child(
                text("Chats")
                    .font(TextStyle {
                        size: 17.0,
                        ..k.ty.body_medium
                    })
                    .color(k.c.text_base),
            ),
        )
        .child(
            IconButton::new(&icons::PLUS)
                .variant(IconButtonVariant::Ghost)
                .label("New chat")
                .help("New chat (⌘N)")
                .on_press(State::new_chat)
                .build(k),
        )
        .child(
            IconButton::new(&icons::SETTINGS)
                .variant(IconButtonVariant::Ghost)
                .label("Settings")
                .help("Settings (⌘,)")
                .on_press(State::open_settings)
                .build(k),
        )
        .child(
            IconButton::new(&icons::EXPAND)
                .variant(IconButtonVariant::Ghost)
                .label("Open the main window")
                .on_press(State::open_main)
                .build(k),
        )
        .child(hide_button(k));
    let search = search_field(s, k);
    let filter = filter_control(s, k);
    let mut body = col().key("rows").w_full().px(8.0).pb(12.0).gap(1.0);
    if let Some(note) = engine_note(s, k) {
        body = body.child(note.mb(8.0));
    }
    body = match &s.list.chats {
        None if s.list.error.is_some() => {
            body.child(empty(k, s.list.error.as_deref().unwrap_or_default()))
        }
        None => body.child(
            row()
                .w_full()
                .py(24.0)
                .justify_center()
                .child(busy(18.0, k)),
        ),
        Some(chats) => body.children(rows(s, k, chats)),
    };
    col()
        .w_full()
        .h_full()
        .child(header)
        .child(row().w_full().px(12.0).pb(8.0).child(search))
        .child(row().w_full().px(12.0).pb(8.0).child(filter))
        .child(
            col()
                .key("list")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

/// The search over titles, projects and previews; Return opens the first match.
pub(super) fn search_field(s: &State, k: &Kit) -> El {
    TextInput::new(&s.list.query)
        .width(None)
        .height(SEARCH_HEIGHT)
        .placeholder("Search")
        .leading_icon(&icons::SEARCH)
        .clear_button()
        .on_edit(|s: &mut State| &mut s.list.query)
        .on_key(|s: &mut State, key, tx: &mut Tx| {
            // Return opens the first match.
            if key.key != rune::Key::Enter {
                return false;
            }
            let first = filtered(s).first().map(|c| c.id.clone());
            if let Some(id) = first {
                s.open_chat(id);
                tx.blur();
            }
            true
        })
        .build(k)
        .radius(SEARCH_HEIGHT / 2.0)
        .pl(4.0)
        .key("search")
}

/// The filter by status.
pub(super) fn filter_control(s: &State, k: &Kit) -> El {
    let labels = ListFilter::ALL.map(ListFilter::label);
    let picked = ListFilter::ALL.iter().position(|f| *f == s.list.filter);
    SegmentedControl::new(&labels, picked)
        .label("Show")
        .width(None)
        .on_pick(|s: &mut State, i, _: &mut Tx| s.list.filter = ListFilter::ALL[i])
        .build(k)
        .key("filter")
}

pub(super) fn empty(k: &Kit, why: &str) -> El {
    row()
        .w_full()
        .py(24.0)
        .justify_center()
        .child(text(why).font(k.ty.ui).color(k.c.text_muted).wrap())
}

/// The chats the search and the filter keep, in list order.
pub(super) fn filtered(s: &State) -> Vec<&Chat> {
    let q = s.list.query.text.trim().to_lowercase();
    let now = Utc::now();
    s.list
        .chats
        .iter()
        .flatten()
        .filter(|c| keeps(s, c, now) && (q.is_empty() || matches(c, &q)))
        .collect()
}

fn keeps(s: &State, c: &Chat, now: DateTime<Utc>) -> bool {
    match s.list.filter {
        ListFilter::All => true,
        ListFilter::Working => c.is_working(),
        ListFilter::Done => is_done(s, c),
        ListFilter::Recent => now - c.active_at() <= RECENT,
    }
}

/// Whether `c` finished a turn since it was last opened.
pub(super) fn is_done(s: &State, c: &Chat) -> bool {
    matches!(c.status, Status::Idle) && s.prefs.borrow().unseen(c)
}

fn matches(c: &Chat, q: &str) -> bool {
    let project = c.project.as_ref().map_or("", |p| p.name.as_str());
    let hay = [
        c.title(),
        project,
        c.last_message_preview.as_deref().unwrap_or_default(),
    ];
    hay.iter().any(|h| h.to_lowercase().contains(q))
}

/// Why the list shows no chats, if it shows none: `chats` loaded, of which
/// the search and filter keep `shown`.
pub(super) fn none_shown(s: &State, chats: &[Chat], shown: &[&Chat]) -> Option<&'static str> {
    if !shown.is_empty() {
        return None;
    }
    if !s.list.query.text.trim().is_empty() {
        return Some("No chats match your search");
    }
    if chats.is_empty() {
        return Some("No chats yet");
    }
    Some(match s.list.filter {
        ListFilter::All => "No chats yet",
        ListFilter::Working => "No chat is working",
        ListFilter::Done => "No finished chats to look at",
        ListFilter::Recent => "No chats active in the last 15 minutes",
    })
}

/// `chats` by project, keyed by project id (`home` for none), each
/// project where its most recently active chat is.
pub(super) fn groups(chats: Vec<&Chat>) -> Vec<(String, Vec<&Chat>)> {
    let mut groups: Vec<(String, Vec<&Chat>)> = Vec::new();
    for c in chats {
        let key = c
            .project
            .as_ref()
            .map_or_else(|| "home".to_owned(), |p| p.id.clone());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, items)) => items.push(c),
            None => groups.push((key, vec![c])),
        }
    }
    groups
}

fn rows(s: &State, k: &Kit, chats: &[Chat]) -> Vec<El> {
    let shown = filtered(s);
    if let Some(why) = none_shown(s, chats, &shown) {
        return vec![empty(k, why)];
    }
    if !s.list.query.text.trim().is_empty() {
        return shown.into_iter().map(|c| chat_row(s, k, c, true)).collect();
    }
    let mut out = Vec::new();
    for (key, items) in groups(shown) {
        let collapsed = s.prefs.borrow().collapsed.contains(&key);
        out.push(section(k, &key, &items, collapsed));
        if collapsed {
            continue;
        }
        let limit = s.list.more.get(&key).copied().unwrap_or(GROUP_CHATS);
        let hidden = items.len().saturating_sub(limit);
        out.extend(items.iter().take(limit).map(|c| chat_row(s, k, c, false)));
        if hidden > 0 {
            out.push(
                row().key(format!("more-{key}")).w_full().pl(2.0).child(
                    Button::new("Show more")
                        .variant(ButtonVariant::Ghost)
                        .on_press(move |s: &mut State| {
                            let shown = s.list.more.entry(key.clone()).or_insert(GROUP_CHATS);
                            *shown += GROUP_CHATS;
                        })
                        .build(k),
                ),
            );
        }
    }
    out
}

fn section(k: &Kit, key: &str, items: &[&Chat], collapsed: bool) -> El {
    let project = items[0].project.as_ref();
    let live = items.iter().any(|c| c.is_working());
    let toggle = key.to_owned();
    let mut head = row()
        .key(format!("section-{key}"))
        .w_full()
        .gap(8.0)
        .px(6.0)
        .pt(10.0)
        .pb(4.0)
        .center_y()
        .child(tile(k, project, 16.0))
        .child(
            text(project.map_or("No project", |p| p.name.as_str()))
                .font(k.ty.small_medium)
                .color(k.c.text_muted)
                .truncate(),
        )
        .child(
            text(items.len().to_string())
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    if live {
        head = head.child(busy(10.0, k));
    }
    let chevron = icon(&icons::CHEV_DOWN, 14.0)
        .color(k.c.icon_faint)
        .rotate(if collapsed {
            -std::f32::consts::FRAC_PI_2
        } else {
            0.0
        });
    head.child(rune::spacer())
        .child(chevron)
        .on_press(move |s: &mut State| {
            s.prefs.borrow_mut().toggle_collapsed(&toggle);
            s.save_prefs();
        })
}

fn chat_row(s: &State, k: &Kit, c: &Chat, with_project: bool) -> El {
    let t = tones(k);
    let status = c.status;
    let unseen = is_done(s, c);
    let corner = match status {
        Status::Working => row()
            .gap(5.0)
            .center_y()
            .child(busy(10.0, k))
            .child(text("Working").font(k.ty.small).color(t.working)),
        Status::AwaitingInput => labelled(k, dot(t.input), "Input", t.input),
        Status::Errored => labelled(k, dot(t.failed), "Failed", t.failed),
        Status::Interrupted => labelled(k, dot(t.failed), "Stopped", t.failed),
        Status::Idle if unseen => {
            labelled(k, icon(&icons::CHECK, 12.0).color(t.done), "Done", t.done)
        }
        Status::Idle => row().child(
            text(ago(c.active_at(), Utc::now()))
                .font(k.ty.small)
                .color(k.c.text_muted),
        ),
    };
    let model = c.model.as_deref().unwrap_or_default();
    let weight = if unseen { k.ty.ui_medium } else { k.ty.ui };
    let top = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(
            icon(icons::mark(model), 14.0)
                .color(k.c.icon_muted)
                .help(short_model(model).to_owned()),
        )
        .child(
            text(c.title())
                .font(weight)
                .color(k.c.text_base)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(corner);
    let mut item = rune::col().w_full().gap(3.0);
    item = item.child(top);
    if with_project {
        item = item.child(
            row()
                .w_full()
                .gap(5.0)
                .pl(22.0)
                .center_y()
                .child(tile(k, c.project.as_ref(), 12.0))
                .child(faint(
                    k,
                    c.project.as_ref().map_or("No project", |p| p.name.as_str()),
                )),
        );
    }
    let id = c.id.clone();
    k.interactive(
        item.key(format!("chat-{}", c.id))
            .px(8.0)
            .py(7.0)
            .radius(8.0),
    )
    .on_press(move |s: &mut State| s.open_chat(id.clone()))
}

fn labelled(k: &Kit, lead: El, label: &str, color: rune::Color) -> El {
    row()
        .gap(5.0)
        .center_y()
        .child(lead)
        .child(text(label.to_owned()).font(k.ty.small).color(color))
}
