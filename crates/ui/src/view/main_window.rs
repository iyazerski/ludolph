//! The main window: the sidebar beside the chat or the settings, under a
//! toolbar with the page's title and actions.

use std::sync::OnceLock;

use rune::{Cx, El, col, el, row, text};
use rune_kit::shell::{SidebarToggle, SplitView, Toolbar};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use super::{chat, icons, settings, sidebar, toast};
use crate::app::{ChatPage, Page, State};

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let panels = panel_kit(k);
    let mut layer = Vec::new();
    let (pane, bar) = match &s.page {
        Page::Chat(page) => (
            chat::pane(s, page, k, &mut layer),
            toolbar(page, k, &mut layer),
        ),
        Page::Settings(page) => (
            settings::view(s, page, k, &mut layer),
            Toolbar::new().leading(settings_title(k)),
        ),
        Page::List => (el(), Toolbar::new()),
    };
    layer.extend(toast(s, k));
    let split = SplitView::new(pane)
        .sidebar(
            &s.sidebar,
            |s: &mut State| &mut s.sidebar,
            sidebar::view(s, panels),
        )
        .toolbar(bar)
        .titlebar_inset(cx.titlebar_inset())
        .sidebar_toggle(SidebarToggle::new().in_header().shortcut("⌃⌘S"))
        .reduced_motion(cx.is_reduced_motion())
        .build(panels);
    rune_kit::root(split, layer)
}

/// `k` for the panels around the chat. In dark mode they're a grey a little
/// lighter than the chat's, in place of the theme's near-black, so the chat
/// reads as the page and the sidebar as the frame around it.
fn panel_kit(k: &'static Kit) -> &'static Kit {
    static DARK: OnceLock<Kit> = OnceLock::new();
    if !k.dark {
        return k;
    }
    DARK.get_or_init(|| {
        let mut panels = k.clone();
        panels.c.bg_deep = k.c.bg_base.mix(k.c.layer_01, 0.5);
        panels
    })
}

/// Back to the chat, and the page's title.
fn settings_title(k: &Kit) -> El {
    row()
        .gap(6.0)
        .center_y()
        .child(
            IconButton::new(&icons::BACK)
                .variant(IconButtonVariant::GhostMuted)
                .label("Back")
                .help("Back (Esc)")
                .on_press(State::close_settings)
                .build(k),
        )
        .child(text("Settings").font(k.ty.ui_medium).color(k.c.text_base))
}

/// The chat's title and project, then its actions.
fn toolbar(page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> Toolbar {
    let (title, sub) = chat::titles(page);
    let mut titles = col().min_w(0.0).shrink().child(
        text(title)
            .font(k.ty.ui_medium)
            .color(k.c.text_base)
            .truncate(),
    );
    if !sub.is_empty() {
        titles = titles.child(
            text(sub)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate(),
        );
    }
    let mut bar = Toolbar::new().leading(titles);
    if let Some(c) = page.existing() {
        let id = c.id.clone();
        bar = bar
            .trailing(
                IconButton::new(&icons::POP_OUT)
                    .variant(IconButtonVariant::GhostMuted)
                    .label("Open in the quick window")
                    .help("Open in the quick window")
                    .on_press(move |s: &mut State| s.open_in_peer(id.clone()))
                    .build(k),
            )
            .trailing(chat::more_menu(page, k, layer));
    }
    bar
}
