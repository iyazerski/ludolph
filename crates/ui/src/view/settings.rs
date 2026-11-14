//! The settings page: the shortcut and login item, the sidecar model, and
//! the phone's link and notifications.
//!
//! Settings sit in rounded groups. A wide control (picker, field) fills the
//! column on the right of its label, and a compact one (switch, button)
//! sits at the right edge, so every row lines up.

use rune::{El, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxOption, Kit, SegmentedControl, ShortcutRecorder,
    Switch, TextInput, shortcut_error_message,
};

use super::widgets::{busy, dot};
use crate::app::{SettingsPage, State};
use crate::data::{LinkStatus, Notify, PhoneLink};
use crate::labels::short_model;

/// The page's widest, so rows stay readable in a wide window.
const MAX_WIDTH: f32 = 640.0;
/// The space inside a group, around its rows.
const PAD: f32 = 12.0;
const LABEL: f32 = 170.0;
/// A wide control's width.
const CONTROL: f32 = 300.0;
/// The sidecar picker's id for pi's default model; a model's id has a `/`.
const DEFAULT_MODEL: &str = "default";

pub fn view(s: &State, page: &SettingsPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut body = col().w_full().max_w(MAX_WIDTH).gap(8.0).px(24.0).py(20.0);
    match (&page.settings, &page.error) {
        (Some(_), _) => {
            body = body
                .child(section(k, "General"))
                .child(group(k, general(s, page, k)))
                .child(section(k, "Model"))
                .child(group(k, vec![sidecar(s, page, k, layer)]))
                .child(section(k, "Phone"))
                .child(group(k, phone(page, k)));
        }
        (None, Some(why)) => {
            body = body.child(
                text(format!("Couldn't load the settings: {why}"))
                    .font(k.ty.ui)
                    .color(k.c.text_muted)
                    .wrap(),
            );
        }
        (None, None) => {
            body = body.child(
                row()
                    .w_full()
                    .py(24.0)
                    .justify_center()
                    .child(busy(18.0, k)),
            );
        }
    }
    body = body.child(col().w_full().pt(8.0).child(group(k, vec![about(k)])));
    col()
        .w_full()
        .h_full()
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(row().w_full().justify_center().child(body))
}

fn general(s: &State, page: &SettingsPage, k: &Kit) -> Vec<El> {
    let at_login = page.settings.as_ref().is_some_and(|s| s.open_at_login);
    vec![
        wide(
            k,
            "Quick window",
            ShortcutRecorder::new("shortcut", &page.shortcut, |s: &mut State| {
                &mut s
                    .settings_page_mut()
                    .expect("the recorder is on the settings page")
                    .shortcut
            })
            .label("Show the quick window")
            .on_change(|s: &mut State, shortcut, _| s.change_shortcut(shortcut))
            .error_message(shortcut_error_message)
            .width(CONTROL)
            .build(k),
        ),
        compact(
            k,
            "Open at login",
            Some("Ludolph opens hidden when you log in, so the shortcut and the phone work."),
            Switch::new(at_login)
                .hide_label()
                .label("Open at login")
                .on_toggle(|s: &mut State| {
                    s.change_settings(|v| v.open_at_login = !v.open_at_login)
                })
                .build(k),
        ),
    ]
    .into_iter()
    .chain(unregistered(s, page, k))
    .collect()
}

/// A note when the saved shortcut isn't registered, as when another app
/// took it first.
fn unregistered(s: &State, page: &SettingsPage, k: &Kit) -> Option<El> {
    let saved = s.prefs.borrow().shortcut.is_some();
    if !saved || s.hotkey.get().is_some() || page.shortcut.is_recording() {
        return None;
    }
    Some(
        row().w_full().px(PAD).py(8.0).child(
            text("Another app uses this shortcut. Record another to use the quick window.")
                .font(k.ty.text_small)
                .color(k.c.text_muted)
                .wrap(),
        ),
    )
}

/// The model for titles and dictation cleanup, searchable: pi lists dozens.
fn sidecar(s: &State, page: &SettingsPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let current = page
        .settings
        .as_ref()
        .and_then(|v| v.sidecar_model.as_deref());
    let mut options = vec![ComboboxOption::new(DEFAULT_MODEL, "pi's default model")];
    if let Some(models) = s.catalog.ready() {
        options.extend(models.iter().map(|m| {
            let label = m
                .label
                .clone()
                .unwrap_or_else(|| short_model(&m.id).to_owned());
            let mut option = ComboboxOption::new(m.id.clone(), label);
            option.detail = m.id.split_once('/').map_or("", |(p, _)| p).to_owned();
            option
        }));
    }
    let picker = Combobox::new("sidecar", &page.model, &options, |s: &mut State| {
        &mut s
            .settings_page_mut()
            .expect("the picker is on the settings page")
            .model
    })
    .label("Titles and dictation")
    .current(Some(current.unwrap_or(DEFAULT_MODEL)))
    .search_placeholder("Search models")
    .width(CONTROL)
    .on_choose(|s: &mut State, id, _| {
        let model = (id != DEFAULT_MODEL).then(|| id.to_owned());
        s.change_settings(|v| v.sidecar_model = model);
    })
    .build(k, layer);
    with_hint(
        wide(k, "Titles and dictation", picker),
        k,
        "Names new chats and cleans up dictation, without thinking. A small, fast model works best.",
    )
}

fn phone(page: &SettingsPage, k: &Kit) -> Vec<El> {
    let notify = page.settings.as_ref().map_or(Notify::Away, |v| v.notify);
    let notify = wide(
        k,
        "Notifications",
        SegmentedControl::new(
            &["When I'm away", "Always"],
            Some(usize::from(notify == Notify::Always)),
        )
        .width(Some(CONTROL))
        .on_pick(|s: &mut State, i, _| {
            let notify = if i == 0 { Notify::Away } else { Notify::Always };
            s.change_settings(|v| v.notify = notify);
        })
        .build(k),
    );
    let notify = with_hint(
        notify,
        k,
        "Away means two minutes without the keyboard or mouse.",
    );
    let link = match &page.phone.link {
        None => row().w_full().px(PAD).py(12.0).child(busy(14.0, k)),
        Some(Some(link)) if link.status != LinkStatus::Rejected => linked(link, k),
        Some(Some(_)) => {
            return vec![
                code_field(
                    page,
                    k,
                    "The relay refused this Mac. Add it again on the phone, then paste the new code.",
                ),
                notify,
            ];
        }
        Some(None) => code_field(
            page,
            k,
            "In the phone app, tap Add a Mac, then paste its code here.",
        ),
    };
    vec![link, notify]
}

/// The link's relay and whether it's connected, with Unlink.
fn linked(link: &PhoneLink, k: &Kit) -> El {
    let host = link
        .relay
        .split_once("://")
        .map_or(link.relay.as_str(), |(_, host)| host);
    let (color, status) = match link.status {
        LinkStatus::Connected => (k.c.success.icon, format!("Connected to {host}")),
        _ => (k.c.warning.icon, format!("Connecting to {host}…")),
    };
    let mut info = col().grow(1.0).min_w(0.0).gap(2.0).child(
        row()
            .gap(8.0)
            .center_y()
            .child(dot(color))
            .child(text(status).font(k.ty.ui).color(k.c.text_base).truncate()),
    );
    if let Some(why) = &link.error {
        info = info.child(
            text(why.clone())
                .font(k.ty.text_small)
                .color(k.c.text_muted)
                .wrap(),
        );
    }
    row()
        .w_full()
        .px(PAD)
        .py(8.0)
        .min_h(44.0)
        .center_y()
        .gap(12.0)
        .child(info)
        .child(
            Button::new("Unlink")
                .variant(ButtonVariant::Outline)
                .help("Disconnect from the relay. To link again, add the Mac on the phone.")
                .on_press(State::unlink_phone)
                .build(k),
        )
}

/// The field for a link code from the phone's Add a Mac, with Link.
fn code_field(page: &SettingsPage, k: &Kit, hint: &str) -> El {
    let phone = &page.phone;
    let input = TextInput::new(&phone.code)
        .label("Link code")
        .placeholder("Link code")
        .width(None)
        .invalid(phone.error.is_some())
        .on_edit(|s: &mut State| {
            &mut s
                .settings_page_mut()
                .expect("the field is on the settings page")
                .phone
                .code
        })
        .on_submit(|s: &mut State, _| s.link_phone())
        .build(k);
    let link = Button::new("Link")
        .variant(if phone.linking {
            ButtonVariant::Loading
        } else {
            ButtonVariant::Neutral
        })
        .disabled(phone.linking || phone.code.text.trim().is_empty())
        .on_press(State::link_phone)
        .build(k);
    let mut block = col()
        .w_full()
        .px(PAD)
        .py(10.0)
        .gap(6.0)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(col().grow(1.0).min_w(0.0).child(input))
                .child(link),
        )
        .child(
            text(hint.to_owned())
                .font(k.ty.text_small)
                .color(k.c.text_muted)
                .wrap(),
        );
    if let Some(why) = &phone.error {
        block = block.child(
            text(why.clone())
                .font(k.ty.text_small)
                .color(k.c.danger.fg)
                .wrap(),
        );
    }
    block
}

/// The version, and Quit, which stops the windows but not the engine.
fn about(k: &Kit) -> El {
    row()
        .w_full()
        .px(PAD)
        .py(8.0)
        .min_h(44.0)
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .gap(2.0)
                .child(
                    text(format!("Ludolph {}", env!("CARGO_PKG_VERSION")))
                        .font(k.ty.ui_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text("Quitting leaves the engine running for the phone until you log out.")
                        .font(k.ty.text_small)
                        .color(k.c.text_muted)
                        .wrap(),
                ),
        )
        .child(
            Button::new("Quit")
                .variant(ButtonVariant::Outline)
                .shortcut("⌘Q")
                .on_press_tx(|_: &mut State, tx| tx.quit())
                .build(k),
        )
}

/// A group's title, above it.
fn section(k: &Kit, title: &str) -> El {
    col().w_full().pt(8.0).pl(4.0).child(
        text(title.to_owned())
            .font(k.ty.text_small_medium)
            .color(k.c.text_muted),
    )
}

/// `rows` in a rounded box, with a line between each two.
fn group(k: &Kit, rows: Vec<El>) -> El {
    let mut card = col()
        .w_full()
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_muted);
    for (i, line) in rows.into_iter().enumerate() {
        if i > 0 {
            card = card.child(el().w_full().h(1.0).fill(k.c.border_muted));
        }
        card = card.child(line);
    }
    card
}

/// A row with `label` on the left and a wide `control` on its right.
fn wide(k: &Kit, label: &str, control: El) -> El {
    row()
        .w_full()
        .px(PAD)
        .py(8.0)
        .min_h(44.0)
        .center_y()
        .gap(12.0)
        .child(
            text(label.to_owned())
                .font(k.ty.ui)
                .color(k.c.text_base)
                .wrap()
                .w(LABEL),
        )
        .child(row().grow(1.0).justify_end().child(control))
}

/// `line` with `hint` under it, across the row.
fn with_hint(line: El, k: &Kit, hint: &str) -> El {
    col().w_full().child(line).child(
        text(hint.to_owned())
            .font(k.ty.text_small)
            .color(k.c.text_muted)
            .wrap()
            .px(PAD)
            .pb(10.0),
    )
}

/// A row with `label`, and `hint` under it, on the left, and a compact
/// `control` at the right edge.
fn compact(k: &Kit, label: &str, hint: Option<&str>, control: El) -> El {
    let hint = hint.map(|h| {
        text(h.to_owned())
            .font(k.ty.text_small)
            .color(k.c.text_muted)
            .wrap()
    });
    row()
        .w_full()
        .px(PAD)
        .py(8.0)
        .min_h(44.0)
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .gap(2.0)
                .child(
                    text(label.to_owned())
                        .font(k.ty.ui)
                        .color(k.c.text_base)
                        .wrap(),
                )
                .children(hint),
        )
        .child(control)
}
