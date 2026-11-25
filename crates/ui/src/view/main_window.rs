use rune::{Cx, El, col, el, row, text};
use rune_kit::shell::{SidebarToggle, SplitView, Toolbar};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use super::{chat, dialogs, icons, settings, sidebar, theme, toast, widgets};
use crate::app::{ChatPage, Page, State};

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let theme = theme::get(s.prefs.borrow().accent);
    let k = &theme.base;
    let panels = &theme.panels;
    let mut layer = Vec::new();
    let (pane, bar) = match &s.page {
        Page::Chat(page) => (
            chat::pane(s, page, k, &mut layer),
            toolbar(s, page, k, &mut layer),
        ),
        Page::Settings(page) => (
            settings::view(s, page, &theme.controls, &mut layer),
            Toolbar::new().leading(widgets::page(settings_title(k), "settings-title")),
        ),
        Page::List => (el(), Toolbar::new()),
    };
    dialogs::chat_dialog(s, k, &mut layer);
    super::popup_menu(s, k, &mut layer);
    layer.extend(toast(s, k));
    let menu = match s.settings_page() {
        Some(page) => widgets::page(settings::sidebar(page, panels), "settings-menu"),
        None => widgets::page(sidebar::view(s, panels), "chats-menu"),
    };
    let split = SplitView::new(pane)
        .sidebar(&s.sidebar, |s: &mut State| &mut s.sidebar, menu)
        .toolbar(bar)
        .titlebar_inset(cx.titlebar_inset())
        .sidebar_toggle(SidebarToggle::new().in_header().shortcut("⌃⌘S"))
        .reduced_motion(cx.is_reduced_motion())
        .build(panels);
    rune_kit::root(split, layer)
}

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

fn toolbar(s: &State, page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> Toolbar {
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
    let titles_key = match page.existing() {
        Some(chat) => format!("chat-titles-{}", chat.id),
        None => "new-chat-titles".to_owned(),
    };
    let mut bar = Toolbar::new().leading(widgets::page(titles, titles_key));
    if let Some(c) = page.existing() {
        let id = c.id.clone();
        bar = bar
            .trailing(
                IconButton::new(&icons::POP_OUT)
                    .variant(IconButtonVariant::GhostMuted)
                    .label("Open in the quick window")
                    .help("Open in the quick window")
                    .on_press(move |s: &mut State| s.open_in_peer(id.clone()))
                    .build(k)
                    .transition(widgets::swap()),
            )
            .trailing(super::details::toggle(s, k).transition(widgets::swap()))
            .trailing(chat::more_menu(page, k, layer).transition(widgets::swap()));
    }
    bar
}
