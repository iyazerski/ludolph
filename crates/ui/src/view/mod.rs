//! What the window draws, and its keys: the chat list or a chat's page,
//! under the toast.

mod breaks;
mod changes;
mod chat;
mod code;
mod composer;
mod icons;
mod list;
mod widgets;

use rune::{Color, Cx, El, Key, KeyEvent, SystemMaterial, Tx, col};
use rune_kit::{Toast, ToastRegion};

use super::app::{ChatPage, Overlay, Page, State, ToastKind};

pub const CORNER: f32 = 14.0;

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let mut layer = Vec::new();
    let page = match &s.page {
        Page::List => list::view(s, k),
        Page::Chat(page) => chat::view(s, page, k, &mut layer),
    };
    if let Some(t) = &s.toast {
        let mut toast = Toast::new("toast", t.title.clone());
        if !t.body.is_empty() {
            toast = toast.description(t.body.clone());
        }
        if t.kind == ToastKind::Failure {
            toast = toast.icon(k, &icons::ALERT);
        }
        layer.push(
            ToastRegion::new()
                .toast(
                    toast
                        .on_close(|s: &mut State| s.toast = None)
                        .closable()
                        .build(k),
                )
                .build(k),
        );
    }
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

pub fn on_key(s: &mut State, key: &KeyEvent, tx: &mut Tx) -> bool {
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
    match s.chat_mut() {
        Some(page) if page.overlay != Overlay::Closed => page.overlay = Overlay::Closed,
        _ if changes => s.back(),
        _ => s.hide(),
    }
    true
}
