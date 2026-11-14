//! The main window's sidebar: a new chat, the search and status filter, and
//! the chats by project, as the quick window lists them, in source-list rows.

use chrono::Utc;
use rune::{El, el, row};
use rune_kit::shell::{RowStatus, Sidebar, SidebarRow, SidebarSection};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};

use super::icons;
use super::list;
use super::widgets::{busy, dot, engine_note, tones};
use crate::app::{GROUP_CHATS, State};
use crate::data::{Chat, Status};
use crate::labels::ago;

pub fn view(s: &State, k: &Kit) -> El {
    let on_new = s.chat().is_some_and(|p| p.draft().is_some());
    let mut bar = Sidebar::new()
        .label("Chats")
        .header(
            SidebarRow::new("New chat")
                .icon(&icons::PLUS)
                .detail("⌘N")
                .selected(on_new)
                .on_select(|s: &mut State, _| {
                    // Arrows pass over the row too; a draft already shown keeps its text.
                    if s.chat().and_then(|p| p.draft()).is_none() {
                        s.new_chat();
                    }
                })
                .build(k)
                .key("new"),
        )
        .header(list::search_field(s, k).mt(6.0))
        .header(list::filter_control(s, k).mt(4.0))
        .footer(
            SidebarRow::new("Settings")
                .icon(&icons::SETTINGS)
                .detail("⌘,")
                .selected(s.settings_page().is_some())
                .on_select(|s: &mut State, _| s.open_settings())
                .build(k)
                .key("settings"),
        );
    if let Some(note) = engine_note(s, k) {
        bar = bar.child(note.mb(8.0));
    }
    let chats = match &s.list.chats {
        Some(chats) => chats,
        None => {
            let waiting = match &s.list.error {
                Some(why) => list::empty(k, why),
                None => row()
                    .w_full()
                    .py(24.0)
                    .justify_center()
                    .child(busy(18.0, k)),
            };
            return bar.child(waiting).build(k);
        }
    };
    let shown = list::filtered(s);
    if let Some(why) = list::none_shown(s, chats, &shown) {
        return bar.child(list::empty(k, why)).build(k);
    }
    if !s.list.query.text.trim().is_empty() {
        return shown
            .into_iter()
            .fold(bar, |bar, c| bar.child(chat_row(s, k, c, true)))
            .build(k);
    }
    for (key, items) in list::groups(shown) {
        bar = bar.section(section(s, k, key, &items));
    }
    bar.build(k)
}

/// A project's chats, the first [`GROUP_CHATS`] of them until "Show more".
fn section(s: &State, k: &Kit, key: String, items: &[&Chat]) -> SidebarSection {
    let project = items[0].project.as_ref();
    let name = project.map_or("No project", |p| p.name.as_str());
    let toggle = key.clone();
    let mut section = SidebarSection::new(name)
        .key(format!("section-{key}"))
        .count(items.len())
        .collapsed(s.prefs.borrow().collapsed.contains(&key))
        .on_toggle(move |s: &mut State, _| {
            s.prefs.borrow_mut().toggle_collapsed(&toggle);
            s.save_prefs();
        });
    if let Some(p) = project {
        let id = p.id.clone();
        section = section.action(
            IconButton::new(&icons::PLUS)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label(format!("New chat in {name}"))
                .help(format!("New chat in {name}"))
                .on_press(move |s: &mut State| s.new_chat_in(id.clone()))
                .build(k)
                .focusable(false),
        );
    }
    let limit = s.list.more.get(&key).copied().unwrap_or(GROUP_CHATS);
    section = section.rows(items.iter().take(limit).map(|c| chat_row(s, k, c, false)));
    if items.len() > limit {
        section = section.row(
            row().key(format!("more-{key}")).w_full().child(
                Button::new("Show more")
                    .variant(ButtonVariant::Ghost)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut State| {
                        *s.list.more.entry(key.clone()).or_insert(GROUP_CHATS) += GROUP_CHATS;
                    })
                    .build(k),
            ),
        );
    }
    section
}

/// A chat's row: its title, when it was last active, and its status at the
/// end. In search results it also names the project.
fn chat_row(s: &State, k: &Kit, c: &Chat, with_project: bool) -> El {
    let t = tones(k);
    let unseen = list::is_done(s, c);
    let when = ago(c.active_at(), Utc::now());
    let (status, detail) = match c.status {
        Status::Working => (RowStatus::Busy, None),
        Status::AwaitingInput => (RowStatus::Attention, Some("Input".to_owned())),
        Status::Errored => (RowStatus::Other(dot(t.failed)), Some("Failed".to_owned())),
        Status::Interrupted => (RowStatus::Other(dot(t.failed)), Some("Stopped".to_owned())),
        Status::Idle if unseen => (RowStatus::Unread, Some(when)),
        // An empty slot keeps the times in line with the rows that have a status.
        Status::Idle => (RowStatus::Other(el()), Some(when)),
    };
    let id = c.id.clone();
    let mut r = SidebarRow::new(c.title())
        .status(status)
        .selected(s.shows_chat(&c.id))
        .on_select(move |s: &mut State, _| {
            if !s.shows_chat(&id) {
                s.open_chat(id.clone());
            }
        });
    if let Some(d) = detail {
        r = r.detail(d);
    }
    if unseen {
        r = r.title_style(k.ty.ui_medium);
    }
    if with_project {
        r = r.subtitle(c.project.as_ref().map_or("No project", |p| p.name.as_str()));
    }
    r.build(k).key(format!("chat-{}", c.id))
}
