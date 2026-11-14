//! What the windows draw, and their keys: the quick window's chat list or
//! chat page, the main window's sidebar and chat ([`main_window`]), each
//! under the toast.

mod breaks;
mod changes;
mod chat;
mod code;
mod composer;
mod dialogs;
mod icons;
mod list;
mod main_window;
mod settings;
mod sidebar;
mod widgets;

use rune::{Color, Cx, El, Key, KeyEvent, SystemMaterial, Tx, col};
use rune_kit::{Toast, ToastRegion};

use rune_kit::Kit;

use super::app::{ChatPage, Overlay, Page, State, ToastKind, Window};

pub const CORNER: f32 = 14.0;

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    match s.window {
        Window::Quick => quick(s, cx),
        Window::Main => main_window::view(s, cx),
    }
}

fn quick(s: &State, cx: &mut Cx<State>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let mut layer = Vec::new();
    let page = match &s.page {
        // The quick window hands the settings to the main one.
        Page::List | Page::Settings(_) => list::view(s, k),
        Page::Chat(page) => chat::view(s, page, k, &mut layer),
    };
    dialogs::chat_dialog(s, k, &mut layer);
    layer.extend(toast(s, k));
    let (tint, edge) = if k.dark {
        // macOS draws dark windows' rims itself; a light line on top reads as a seam.
        (k.c.bg_base.alpha(0.72), Color::TRANSPARENT)
    } else {
        (Color::WHITE.alpha(0.8), k.c.border_base.alpha(0.6))
    };
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
                .fill(tint)
                .border(1.0, edge)
                .child(page),
        );
    rune_kit::root(body, layer)
}

/// The window's toast, if it shows one.
fn toast(s: &State, k: &Kit) -> Option<El> {
    let t = s.toast.as_ref()?;
    let mut toast = Toast::new("toast", t.title.clone());
    if !t.body.is_empty() {
        toast = toast.description(t.body.clone());
    }
    if t.kind == ToastKind::Failure {
        toast = toast.icon(k, &icons::ALERT);
    }
    Some(
        ToastRegion::new()
            .toast(
                toast
                    .on_close(|s: &mut State| s.toast = None)
                    .closable()
                    .build(k),
            )
            .build(k),
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
    if let Some(page) = s.settings_page_mut() {
        // The recorder takes every key while it records, Escape included.
        if page.shortcut.is_recording() {
            return false;
        }
        if key.key == Key::Escape && page.model.is_open() {
            page.model.close();
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
    // Its hunk and file keys, and copying its selected lines.
    if changes && s.diff_mut().key(key, tx) {
        return true;
    }
    if key.key != Key::Escape {
        return false;
    }
    let quick = s.window == Window::Quick;
    match s.chat_mut() {
        Some(page) if page.overlay != Overlay::Closed => page.overlay = Overlay::Closed,
        _ if changes => s.back(),
        // Escape puts the floating window away; a regular window stays.
        _ if quick => s.hide(),
        _ => return false,
    }
    true
}
