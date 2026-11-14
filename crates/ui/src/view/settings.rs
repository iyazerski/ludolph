//! The settings page: the shortcut and login item, the sidecar model, and
//! the phone's link and notifications.
//!
//! Settings sit in rounded groups. A wide control (picker, field) fills the
//! column on the right of its label, and a compact one (switch, button)
//! sits at the right edge, so every row lines up.

use std::sync::OnceLock;

use rune::{El, SurfaceFit, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxOption, Kit, SegmentedControl, ShortcutRecorder,
    Switch, TextInput, shortcut_error_message,
};

use super::widgets::{busy, dot};
use crate::app::{QR_SIZE, SettingsPage, State, Update};
use crate::data::{LinkStatus, Notify, Pairing, RelayLink};
use crate::labels::short_model;

/// The page's widest, so rows stay readable in a wide window.
const MAX_WIDTH: f32 = 640.0;
/// The space inside a group, around its rows.
const PAD: f32 = 12.0;
const LABEL: f32 = 170.0;
/// A wide control's width.
const CONTROL: f32 = 300.0;
/// The sidecar picker's id for pi's default model, which follows pi's
/// settings; a model's id has a `/`.
const DEFAULT_MODEL: &str = "default";

pub fn view(s: &State, page: &SettingsPage, k: &'static Kit, layer: &mut Vec<El>) -> El {
    let k = controls_kit(k);
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
    body = body.child(col().w_full().pt(8.0).child(group(k, vec![about(page, k)])));
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
    let expanded = page.settings.as_ref().is_some_and(|s| s.expand_tools);
    let tools = wide(
        k,
        "Tool calls",
        SegmentedControl::new(&["Collapsed", "Expanded"], Some(usize::from(expanded)))
            .width(Some(CONTROL))
            .on_pick(|s: &mut State, i, _| s.change_settings(|v| v.expand_tools = i == 1))
            .build(k),
    );
    let tools = with_hint(
        tools,
        k,
        "Whether the agent's commands, edits and thinking show open while it works. They fold once it writes again.",
    );
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
        tools,
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
    let default = s.catalog.default_model().map(|id| s.catalog.label(id));
    let default = default.map_or("Default model".to_owned(), |name| {
        format!("{name} (default)")
    });
    let mut options = vec![ComboboxOption::new(DEFAULT_MODEL, default)];
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
    let mut out = Vec::new();
    match &page.phone.state {
        None => out.push(row().w_full().px(PAD).py(12.0).child(busy(14.0, k))),
        Some(state) => {
            let pairing = state.pairing.as_ref();
            match state
                .link
                .as_ref()
                .filter(|l| l.status != LinkStatus::Rejected)
            {
                Some(link) => out.push(if page.phone.confirm_unlink {
                    confirm_unlink(k)
                } else {
                    linked(link, pairing.is_none(), k)
                }),
                None => {
                    out.push(relay_field(page, pairing.is_none(), k));
                    if pairing.is_none() {
                        let rejected = state.link.is_some();
                        out.push(link_row(k, rejected));
                    }
                }
            }
            if let Some(pairing) = pairing {
                out.push(pairing_block(page, pairing, k));
            }
        }
    }
    if let Some(why) = &page.phone.error {
        out.push(
            row().w_full().px(PAD).py(8.0).child(
                text(why.clone())
                    .font(k.ty.text_small)
                    .color(k.c.danger.fg)
                    .wrap(),
            ),
        );
    }
    out.push(notify);
    out
}

/// The link's relay and whether it's connected, with buttons to link
/// another phone (unless one is linking) and to unlink.
fn linked(link: &RelayLink, can_link: bool, k: &Kit) -> El {
    let (color, status) = match link.status {
        LinkStatus::Connected => (
            k.c.success.icon,
            format!("Connected to {}", host(&link.relay)),
        ),
        _ => (
            k.c.warning.icon,
            format!("Connecting to {}…", host(&link.relay)),
        ),
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
    let mut buttons = row().gap(8.0);
    if can_link {
        buttons = buttons.child(
            Button::new("Link a phone")
                .variant(ButtonVariant::Neutral)
                .help("Show a QR code to scan with another phone")
                .on_press(State::link_phone)
                .build(k),
        );
    }
    buttons = buttons.child(
        Button::new("Unlink")
            .variant(ButtonVariant::Outline)
            .help("Disconnect this Mac from the relay, so no phone reaches it")
            .on_press(|s: &mut State| s.confirm_unlink(true))
            .build(k),
    );
    status_row(info, buttons)
}

/// Unlink's second step, in the link's place: what it does, and whether to.
fn confirm_unlink(k: &Kit) -> El {
    let info = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(text("Unlink this Mac?").font(k.ty.ui).color(k.c.text_base))
        .child(
            text("No phone reaches it until you link one again, and the relay forgets it.")
                .font(k.ty.text_small)
                .color(k.c.text_muted)
                .wrap(),
        );
    let buttons = row()
        .gap(8.0)
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .on_press(|s: &mut State| s.confirm_unlink(false))
                .build(k),
        )
        .child(
            Button::new("Unlink")
                .variant(ButtonVariant::Danger)
                .on_press(State::unlink_phone)
                .build(k),
        );
    status_row(info, buttons)
}

/// The phone link's row: what it says, then its buttons.
fn status_row(info: El, buttons: El) -> El {
    row()
        .w_full()
        .px(PAD)
        .py(8.0)
        .min_h(44.0)
        .center_y()
        .gap(12.0)
        .child(info)
        .child(buttons)
}

/// `relay` without its scheme, as people write an address.
fn host(relay: &str) -> &str {
    relay.split_once("://").map_or(relay, |(_, host)| host)
}

/// The relay to link through, which only a Mac that isn't linked yet picks.
fn relay_field(page: &SettingsPage, editable: bool, k: &Kit) -> El {
    let input = TextInput::new(&page.phone.relay)
        .label("Relay")
        .placeholder("https://")
        .width(Some(CONTROL))
        .disabled(!editable)
        .on_edit(|s: &mut State| {
            &mut s
                .settings_page_mut()
                .expect("the field is on the settings page")
                .phone
                .relay
        })
        .on_submit(|s: &mut State, _| s.link_phone())
        .build(k);
    with_hint(
        wide(k, "Relay", input),
        k,
        "The server your phone reaches this Mac through. Change it if you run your own.",
    )
}

/// The button that starts linking, for a Mac that isn't linked.
fn link_row(k: &Kit, rejected: bool) -> El {
    let hint = if rejected {
        "The relay forgot this Mac. Link your phone again."
    } else {
        "Scan a QR code with your phone's camera to use Ludolph there."
    };
    compact(
        k,
        "Phone",
        Some(hint),
        Button::new("Link a phone")
            .variant(ButtonVariant::Neutral)
            .on_press(State::link_phone)
            .build(k),
    )
}

/// A phone linking: getting a code, its QR code, or why it failed.
fn pairing_block(page: &SettingsPage, pairing: &Pairing, k: &Kit) -> El {
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Outline)
        .on_press(State::stop_linking_phone)
        .build(k);
    let note = |s: &str| {
        text(s.to_owned())
            .font(k.ty.text_small)
            .color(k.c.text_muted)
            .wrap()
    };
    let body = match pairing {
        Pairing::Waiting { .. } => {
            // White around the code, like paper, so cameras find it in dark mode too.
            let mut tile = el()
                .size(QR_SIZE + 16.0, QR_SIZE + 16.0)
                .radius(10.0)
                .fill(rune::Color::WHITE)
                .center();
            if let Some((_, qr)) = &page.phone.qr {
                tile = tile.child(
                    el().size(QR_SIZE, QR_SIZE)
                        .pixels_fit(qr.pixels(), SurfaceFit::Fill),
                );
            }
            col()
                .items_center()
                .gap(10.0)
                .child(tile)
                .child(text("Scan with your phone's camera").font(k.ty.ui).color(k.c.text_base))
                .child(note("Open the link, then sign in or create a passkey. The code works for 10 minutes."))
                .child(cancel)
        }
        Pairing::Failed { error } => col()
            .items_center()
            .gap(10.0)
            .child(
                text(format!("Couldn't link a phone: {error}"))
                    .font(k.ty.ui)
                    .color(k.c.danger.fg)
                    .wrap(),
            )
            .child(
                row().gap(8.0).child(cancel).child(
                    Button::new("Try again")
                        .variant(ButtonVariant::Neutral)
                        .on_press(State::link_phone)
                        .build(k),
                ),
            ),
        Pairing::Starting | Pairing::Linked => col()
            .items_center()
            .gap(10.0)
            .child(busy(18.0, k))
            .child(note("Getting a code from the relay…")),
    };
    row().w_full().px(PAD).py(16.0).justify_center().child(body)
}

/// The version and where updating stands, with the update and Quit buttons.
/// Quit stops the windows but not the engine.
fn about(page: &SettingsPage, k: &Kit) -> El {
    let (status, failed) = match &page.update {
        Update::Checking => ("Checking for updates…".to_owned(), false),
        Update::Latest => ("This is the latest version.".to_owned(), false),
        Update::Available(version) => (
            format!("Version {version} is out. Updating restarts Ludolph and stops running chats."),
            false,
        ),
        Update::Installing(version) => (format!("Installing version {version}…"), false),
        Update::Failed(why) => (why.clone(), true),
    };
    let update = match &page.update {
        Update::Available(version) => Some(
            Button::new(format!("Update to {version}"))
                .variant(ButtonVariant::Neutral)
                .on_press(State::install_update)
                .build(k),
        ),
        Update::Latest | Update::Failed(_) => Some(
            Button::new("Check again")
                .variant(ButtonVariant::Outline)
                .on_press(State::check_update)
                .build(k),
        ),
        Update::Checking | Update::Installing(_) => Some(busy(16.0, k)),
    };
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
                    text(format!("Ludolph {}", crate::VERSION))
                        .font(k.ty.ui_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text(status)
                        .font(k.ty.text_small)
                        .color(if failed {
                            k.c.danger.fg
                        } else {
                            k.c.text_muted
                        })
                        .wrap(),
                ),
        )
        .children(update)
        .child(
            Button::new("Quit")
                .variant(ButtonVariant::Outline)
                .shortcut("⌘Q")
                .help(
                    "Close the windows. The engine keeps running for the phone until you log out.",
                )
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

/// `k` with the fields, pickers and segmented controls on the chat
/// composer's surface, so they read as fields on the groups' cards rather
/// than holes in them.
fn controls_kit(k: &'static Kit) -> &'static Kit {
    static LIGHT: OnceLock<Kit> = OnceLock::new();
    static DARK: OnceLock<Kit> = OnceLock::new();
    let cell = if k.dark { &DARK } else { &LIGHT };
    cell.get_or_init(|| {
        let mut controls = k.clone();
        controls.c.bg_base = k.c.bg_base.mix(k.c.text_base, 0.035);
        controls
    })
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
