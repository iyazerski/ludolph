mod breaks;
mod changes;
mod chat;
mod code;
mod composer;
mod details;
mod dialogs;
mod icons;
mod list;
mod main_window;
mod model_selector;
mod project;
mod queue;
mod settings;
#[cfg(windows)]
mod setup;
mod sidebar;
mod theme;
mod widgets;

use rune::{
    Cx, Edge, El, Key, KeyEvent, SystemMaterial, TextStyle, Transition, Tx, col, row, text,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Menu, Toast,
};

use super::app::{ChatPage, Overlay, Page, State, ToastKind, Window};
use super::prefs::TextSize;

pub const CORNER: f32 = 14.0;

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    cx.zoom(s.prefs.borrow().text_size.zoom());
    #[cfg(windows)]
    if s.window == Window::Main
        && !matches!(s.page, Page::Settings(_))
        && let Some(status) = &s.wsl_setup
        && !matches!(status, crate::wsl::Status::Ready)
    {
        let k = &theme::get(s.prefs.borrow().accent).base;
        return rune_kit::root(setup::view(status, k), toast(s, k));
    }
    match s.window {
        Window::Quick => quick(s),
        Window::Main => main_window::view(s, cx),
    }
}

fn quick(s: &State) -> El {
    let theme = theme::get(s.prefs.borrow().accent);
    let k = &theme.base;
    let mut layer = Vec::new();
    let page = match &s.page {
        Page::List | Page::Settings(_) => widgets::page(list::view(s, k), "list-page"),
        Page::Chat(page) => chat::view(s, page, k, &mut layer),
    };
    dialogs::chat_dialog(s, k, &mut layer);
    popup_menu(s, k, &mut layer);
    layer.extend(toast(s, k));
    // Rune drops the fill of an element that hosts a native material, so the tint goes on a child.
    let body = col()
        .key("window")
        .w_full()
        .h_full()
        .radius(CORNER)
        .system_material(SystemMaterial::Popover)
        .child(
            col()
                .w_full()
                .h_full()
                .radius(CORNER)
                .fill(k.c.bg_base.alpha(0.65))
                .child(page),
        );
    rune_kit::root(body, layer)
}

pub(super) fn popup_menu(s: &State, k: &Kit, layer: &mut Vec<El>) {
    if let Some(req) = &s.popup_menu {
        let _ = Menu::from_popup_menu(
            "popup-fallback",
            req,
            |s: &mut State, picked, tx: &mut Tx| {
                s.popup_menu = None;
                tx.resolve_popup_menu(picked);
            },
        )
        .build(k, layer);
    }
}

fn notice_toast(
    key: &'static str,
    title: String,
    action: &'static str,
    on_action: impl Fn(&mut State, &mut Tx) + 'static,
    on_close: impl Fn(&mut State) + 'static,
    k: &Kit,
) -> El {
    let root = row()
        .key(key)
        .role("status")
        .label(title.clone())
        .max_w(420.0)
        .pl(14.0)
        .pr(10.0)
        .py(8.0)
        .gap(10.0)
        .center_y()
        .radius(18.0)
        .fill(k.c.layer_01)
        .child(
            text(title)
                .key("title")
                .font(TextStyle {
                    line_height: 20.0,
                    ..k.ty.ui_medium
                })
                .color(k.c.text_base)
                .min_w(0.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(
            Button::new(action)
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Regular)
                .on_press_tx(on_action)
                .build(k),
        )
        .child(
            IconButton::new(k.glyph.close)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label("Close")
                .on_press(on_close)
                .build(k),
        );
    k.elevate(root, &k.elevation.floating)
        .transition(Transition::move_edge(Edge::Top))
        .motion(rune::Spring::new(0.28, 0.0))
}

fn toast(s: &State, k: &Kit) -> Option<El> {
    let updated = s.updated_notice.as_ref().map(|version| {
        notice_toast(
            "updated",
            format!("Ludolph updated to {version}"),
            "Release notes",
            State::open_updated_release_notes,
            State::dismiss_updated,
            k,
        )
    });
    let release = s.release_notice.as_ref().map(|version| {
        notice_toast(
            "release",
            format!("Ludolph {version} is out"),
            "Update",
            State::install_release,
            State::dismiss_release,
            k,
        )
    });
    let note = s.toast.as_ref().map(|t| {
        let mut toast = Toast::new("toast", t.title.clone());
        if !t.body.is_empty() {
            toast = toast.description(t.body.clone());
        }
        if t.kind == ToastKind::Failure {
            toast = toast.icon(k, &icons::ALERT);
        }
        toast
            .on_close(|s: &mut State| s.toast = None)
            .build(k)
            .w_full()
            .max_w(320.0)
            .radius(18.0)
            .transition(Transition::move_edge(Edge::Top))
    });
    if updated.is_none() && release.is_none() && note.is_none() {
        return None;
    }
    Some(
        col()
            .key("toast-region")
            .role("region")
            .label("Notifications")
            .pin(Some(16.0), Some(16.0), Some(16.0), None)
            .items_center()
            .gap(8.0)
            .children(updated.into_iter().chain(release).chain(note)),
    )
}

pub fn on_key(s: &mut State, key: &KeyEvent, tx: &mut Tx) -> bool {
    if s.chat_dialog.is_some() {
        if key.key == Key::Escape {
            s.close_chat_dialog();
            return true;
        }
        return false;
    }
    if key.key == Key::Escape
        && let Some(draft) = s.chat_mut().and_then(|page| page.draft_mut())
        && draft.project_menu.open
    {
        draft.project_menu.open = false;
        draft.project_menu.search.focused = false;
        return true;
    }
    if let Some(page) = s.settings_page_mut() {
        // The recorder takes every key while it records, Escape included.
        if page.shortcut.is_recording() {
            return false;
        }
        if key.key == Key::Escape && page.close_open_picker() {
            return true;
        }
        if key.key == Key::Escape || key.is_command('[') {
            s.close_settings();
            return true;
        }
    }
    if key.is_command(',') {
        s.open_settings();
        return true;
    }
    if s.window == Window::Main {
        if rune_kit::shell::is_sidebar_toggle(key) {
            s.sidebar.toggle();
            return true;
        }
        if key.is_command('f') {
            // The search field lives in the chat list, which Settings replaces.
            s.close_settings();
            s.sidebar.set_open(true);
            if let Some(page) = s.chat_mut() {
                page.composer.focused = false;
            }
            s.list.query.focused = true;
            return true;
        }
    }
    if key.is_command('w') {
        s.hide();
        return true;
    }
    if key.is_command('q') {
        tx.quit();
        return true;
    }
    let size = s.prefs.borrow().text_size;
    if key.is_command('=') || key.is_command('+') {
        s.set_text_size(size.step(1));
        return true;
    }
    if key.is_command('-') {
        s.set_text_size(size.step(-1));
        return true;
    }
    if key.is_command('0') {
        s.set_text_size(TextSize::Default);
        return true;
    }
    if key.is_command('n') {
        s.new_chat();
        return true;
    }
    if key.is_command('[') && s.chat().is_some() {
        s.back();
        return true;
    }
    let changes = s
        .chat()
        .and_then(ChatPage::existing)
        .is_some_and(|c| c.changes.is_some());
    if changes && s.diff_mut().key(key, tx) {
        return true;
    }
    if key.key != Key::Escape {
        return false;
    }
    let quick = s.window == Window::Quick;
    let details = s
        .chat()
        .and_then(ChatPage::existing)
        .is_some_and(|chat| chat.details_open);
    match s.chat_mut() {
        Some(page) if page.model_picker.is_open() => page.model_picker.close(),
        Some(page) if page.overlay != Overlay::Closed => page.overlay = Overlay::Closed,
        Some(page) if page.queue_edit.is_some() => page.cancel_queue_edit(),
        _ if changes || details => s.back(),
        _ if quick => s.hide(),
        _ => return false,
    }
    true
}
