//! The main window's sidebar: a new chat and the search, then the chats by
//! project, each under its folder.

use rune::{El, col, el, row, text};
use rune_kit::shell::{ROW_HEIGHT, RowStatus, Sidebar, SidebarRow};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};

use super::icons;
use super::list;
use super::widgets::{busy, dot, engine_note, tones};
use crate::app::{GROUP_CHATS, State};
use crate::data::{Chat, Status};

/// How far a chat's title sits in from its project's folder: past the
/// folder icon and its gap, so the titles line up with the project's name.
const NAME_X: f32 = 16.0 + 6.0;

pub fn view(s: &State, k: &Kit) -> El {
    let on_new = s.chat().is_some_and(|p| p.draft().is_some());
    let search = if s.list.query.focused || !s.list.query.text.is_empty() {
        list::search_field(s, k)
    } else {
        SidebarRow::new("Search")
            .icon(&icons::SEARCH)
            .detail("⌘F")
            .on_select(|s: &mut State, _| s.list.query.focused = true)
            .build(k)
            .key("search-row")
    };
    let mut bar = Sidebar::new()
        .label("Chats")
        .header(
            SidebarRow::new("New chat")
                .icon(&icons::NEW_CHAT)
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
        .header(search)
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
    bar = bar.child(
        text("Projects")
            .font(k.ty.small)
            .color(k.c.text_faint)
            .px(8.0)
            .pt(10.0)
            .pb(4.0)
            .key("projects"),
    );
    for (key, items) in list::groups(shown) {
        bar = bar.child(project(s, k, key, &items));
    }
    bar.build(k)
}

/// A project's folder, which opens and closes it, and its chats, the first
/// [`GROUP_CHATS`] of them until "Show more".
fn project(s: &State, k: &Kit, key: String, items: &[&Chat]) -> El {
    let project = items[0].project.as_ref();
    let name = project.map_or("No project", |p| p.name.as_str());
    let collapsed = s.prefs.borrow().collapsed.contains(&key);
    let toggle = key.clone();
    let mut folder = SidebarRow::new(name)
        .icon(if collapsed {
            &icons::FOLDER
        } else {
            &icons::FOLDER_OPEN
        })
        .on_select(move |s: &mut State, _| {
            s.prefs.borrow_mut().toggle_collapsed(&toggle);
            s.save_prefs();
        });
    if let Some(p) = project {
        let id = p.id.clone();
        folder = folder.trailing(
            IconButton::new(&icons::PLUS)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label(format!("New chat in {name}"))
                .help(format!("New chat in {name}"))
                .on_press(move |s: &mut State| s.new_chat_in(id.clone()))
                .build(k)
                // Pointer only, and only under it, as the row's other hover controls.
                .focusable(false)
                .group_opacity(0.0, 1.0),
        );
    }
    // Pointer only: with selection following focus, the arrows would open and
    // close it as they passed.
    let folder = folder
        .build(k)
        .focusable(false)
        .expanded(!collapsed)
        .key("folder");
    let mut out = col()
        .key(format!("project-{key}"))
        .w_full()
        .gap(2.0)
        .pb(8.0)
        .motion(k.motion.disclosure)
        .child(folder);
    if collapsed {
        return out;
    }
    let limit = s.list.more.get(&key).copied().unwrap_or(GROUP_CHATS);
    out = out.children(items.iter().take(limit).map(|c| chat_row(s, k, c, false)));
    if items.len() > limit {
        let more = row()
            .w_full()
            .h(ROW_HEIGHT)
            .pl(8.0 + NAME_X)
            .center_y()
            .radius(k.radius.md)
            .child(text("Show more").font(k.ty.ui).color(k.c.text_faint));
        out = out.child(
            k.interactive(more)
                .key("more")
                .role("button")
                .label(format!("Show more chats in {name}"))
                .on_press(move |s: &mut State| {
                    *s.list.more.entry(key.clone()).or_insert(GROUP_CHATS) += GROUP_CHATS;
                }),
        );
    }
    out
}

/// A chat's row: its title under its project's name, and its status at the
/// end. In search results it names the project under the title instead.
fn chat_row(s: &State, k: &Kit, c: &Chat, with_project: bool) -> El {
    let t = tones(k);
    let unseen = list::is_done(s, c);
    let (status, said) = match c.status {
        Status::Working => (Some(RowStatus::Busy), None),
        Status::AwaitingInput => (Some(RowStatus::Attention), Some("Needs your input")),
        Status::Errored => (Some(RowStatus::Other(dot(t.failed))), Some("Failed")),
        Status::Interrupted => (Some(RowStatus::Other(dot(t.failed))), Some("Stopped")),
        Status::Idle if unseen => (Some(RowStatus::Unread), None),
        Status::Idle => (None, None),
    };
    let id = c.id.clone();
    let mut r = SidebarRow::new(c.title())
        .selected(s.shows_chat(&c.id))
        .on_select(move |s: &mut State, _| {
            if !s.shows_chat(&id) {
                s.open_chat(id.clone());
            }
        });
    if let Some(status) = status {
        r = r.status(status);
    }
    if unseen {
        r = r.title_style(k.ty.ui_medium);
    }
    if with_project {
        r = r.subtitle(c.project.as_ref().map_or("No project", |p| p.name.as_str()));
    } else {
        // An empty lead the folder icon's size, so the title lines up with the project's name.
        r = r.avatar(el());
    }
    let mut out = r.build(k).key(format!("chat-{}", c.id));
    if let Some(said) = said {
        out = out.help(said);
    }
    out
}
