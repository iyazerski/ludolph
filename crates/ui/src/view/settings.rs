use rune::{Color, El, Icon, SurfaceFit, Tx, col, el, row, text};
use rune_kit::shell::{Sidebar, SidebarRow};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxOption, Kit, Placement, SegmentedControl, Select,
    SelectAppearance, ShortcutRecorder, Switch, TextInput, shortcut_error_message,
};

use super::widgets::{self, busy, dot, model_options};
use super::{icons, theme};
use crate::app::{Outcome, QR_SIZE, Section, SettingsPage, State, Update};
use crate::data::{
    GuiTool, LinkStatus, Notify, Pairing, RelayLink, SendBehavior, ToolStatus, levels_of,
};
use crate::labels::reasoning_name;
use crate::prefs::{Accent, TextSize};

const MAX_WIDTH: f32 = 640.0;
const PAD: f32 = 12.0;
const LABEL: f32 = 170.0;
const CONTROL: f32 = 300.0;
/// The reasoning picker next to a model picker; the pair is `CONTROL` wide.
const THINKING: f32 = 108.0;
const GAP: f32 = 8.0;
/// The sidecar picker's id for pi's default model, which follows pi's
/// settings; a model's id has a `/`.
const DEFAULT_MODEL: &str = "default";

pub fn view(s: &State, page: &SettingsPage, k: &'static Kit, layer: &mut Vec<El>) -> El {
    let content = match page.section {
        Section::General => loaded(page, k, || group(k, general(s, page, k))),
        Section::Appearance => group(k, vec![accents(s, k), text_size(s, k)]),
        Section::AgentTools => loaded(page, k, || group(k, agent_tools(page, k))),
        Section::Model => loaded(page, k, || {
            col()
                .w_full()
                .gap(12.0)
                .child(group(
                    k,
                    vec![sidecar(s, page, k, layer), gui_model(s, page, k, layer)],
                ))
                .child(caption(k, "Chat model picker"))
                .child(group(k, chat_models(s, page, k)))
        }),
        Section::Phone => loaded(page, k, || group(k, phone(page, k))),
        Section::About => group(k, vec![about(page, k)]),
    };
    let body = col()
        .w_full()
        .max_w(MAX_WIDTH)
        .gap(12.0)
        .px(24.0)
        .py(20.0)
        .child(
            text(title(page.section))
                .font(k.ty.body_medium)
                .color(k.c.text_base)
                .pl(4.0),
        )
        .child(content);
    // Keyed by section so each one opens scrolled to its top.
    widgets::page(
        col()
            .w_full()
            .h_full()
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(row().w_full().justify_center().child(body)),
        key(page.section),
    )
}

/// The settings sections, shown in the main sidebar in place of the chats.
pub fn sidebar(page: &SettingsPage, k: &Kit) -> El {
    Section::ALL
        .into_iter()
        .fold(Sidebar::new().label("Settings"), |bar, section| {
            bar.child(
                SidebarRow::new(title(section))
                    .icon(icon(section))
                    .selected(page.section == section)
                    .on_select(move |s: &mut State, _| s.show_settings_section(section))
                    .build(k)
                    .key(key(section)),
            )
        })
        .build(k)
}

fn title(section: Section) -> &'static str {
    match section {
        Section::General => "General",
        Section::Appearance => "Appearance",
        Section::AgentTools => "Agent tools",
        Section::Model => "Model",
        Section::Phone => "Phone",
        Section::About => "About",
    }
}

fn key(section: Section) -> &'static str {
    match section {
        Section::General => "general",
        Section::Appearance => "appearance",
        Section::AgentTools => "agent-tools",
        Section::Model => "model",
        Section::Phone => "phone",
        Section::About => "about",
    }
}

fn icon(section: Section) -> &'static Icon {
    match section {
        Section::General => &icons::SLIDERS,
        Section::Appearance => &icons::APPEARANCE,
        Section::AgentTools => &icons::EXTENSIONS,
        Section::Model => &icons::MODELS,
        Section::Phone => &icons::PHONE,
        Section::About => &icons::INFO,
    }
}

/// A section that needs the engine's settings, once they load.
fn loaded(page: &SettingsPage, k: &Kit, content: impl FnOnce() -> El) -> El {
    match (&page.settings, &page.error) {
        (Some(_), _) => content(),
        (None, Some(why)) => text(format!("Couldn't load the settings: {why}"))
            .font(k.ty.ui)
            .color(k.c.text_muted)
            .wrap()
            .pl(4.0),
        (None, None) => row()
            .w_full()
            .py(24.0)
            .justify_center()
            .child(busy(18.0, k)),
    }
}

fn accents(s: &State, k: &Kit) -> El {
    let current = s.prefs.borrow().accent;
    let choices = row()
        .role("radiogroup")
        .label("Accent color")
        .gap(8.0)
        .children(Accent::ALL.map(|accent| {
            let color = theme::get(accent).base.c.bg_accent;
            el().key(accent.name())
                .role("radio")
                .label(accent.name())
                .checked(current == accent)
                .focusable(true)
                .focus_ring(2.0, 2.0, k.c.border_focus)
                .size(28.0, 28.0)
                .radius(14.0)
                .border(
                    2.0,
                    if current == accent {
                        color
                    } else {
                        Color::TRANSPARENT
                    },
                )
                .child(el().size(18.0, 18.0).radius(9.0).fill(color).a11y_hidden())
                .items_center()
                .justify_center()
                .help(accent.name())
                .on_press(move |s: &mut State| {
                    s.prefs.borrow_mut().accent = accent;
                    s.save_prefs();
                })
        }));
    with_hint(
        compact(k, "Accent color", None, choices),
        k,
        "Colors user messages, links and inline code in both windows.",
    )
}

fn text_size(s: &State, k: &Kit) -> El {
    let current = s.prefs.borrow().text_size;
    let picker = SegmentedControl::new(
        &TextSize::ALL.map(TextSize::name),
        TextSize::ALL.iter().position(|&size| size == current),
    )
    .width(Some(CONTROL))
    .on_pick(|s: &mut State, i, _| s.set_text_size(TextSize::ALL[i]))
    .build(k);
    with_hint(
        wide(k, "Text size", picker),
        k,
        "Scales text, icons and spacing in both windows. ⌘+ and ⌘− change it, and ⌘0 resets it.",
    )
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
        with_hint(
            wide(k, "Send while working",
                SegmentedControl::new(&["Queue", "Steer"], Some(usize::from(
                    page.settings.as_ref().is_some_and(|settings| settings.send_behavior == SendBehavior::Steer)
                )))
                    .width(Some(CONTROL))
                    .on_pick(|s: &mut State, index, _| s.change_settings(|settings| {
                        settings.send_behavior = if index == 0 { SendBehavior::Queue } else { SendBehavior::Steer };
                    }))
                    .build(k)
            ), k,
            "Queue waits for a new turn and keeps messages editable. Steer sends instructions after the current tool step.",
        ),
    ]
    .into_iter()
    .chain(unregistered(s, page, k))
    .collect()
}

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
        options.extend(model_options(models));
    }
    let picker = Combobox::new("sidecar", &page.model, &options, |s: &mut State| {
        &mut s
            .settings_page_mut()
            .expect("the picker is on the settings page")
            .model
    })
    .label("Sidecar model")
    .current(Some(current.unwrap_or(DEFAULT_MODEL)))
    .search_placeholder("Search models")
    .width(CONTROL)
    .on_choose(|s: &mut State, id, _| {
        let model = (id != DEFAULT_MODEL).then(|| id.to_owned());
        s.change_settings(|v| v.sidecar_model = model);
    })
    .build(k, layer);
    with_hint(
        wide(k, "Sidecar model", picker),
        k,
        "Names new chats and cleans up dictation, without thinking. A small, fast model works best.",
    )
}

fn chat_models(s: &State, page: &SettingsPage, k: &Kit) -> Vec<El> {
    let Some(models) = s.catalog.ready() else {
        return vec![note(k, "Loading models…")];
    };
    let picked = page
        .settings
        .as_ref()
        .map_or(&[][..], |v| v.chat_models.as_slice());
    let offered = |id: &String| picked.is_empty() || picked.contains(id);
    let count = models.iter().filter(|m| offered(&m.id)).count();
    let mut rows: Vec<El> = models
        .iter()
        .map(|m| {
            let on = offered(&m.id);
            let name = s.catalog.label(&m.id);
            let provider = m.id.split_once('/').map_or("", |(p, _)| p);
            let id = m.id.clone();
            let pick = Switch::new(on)
                .hide_label()
                .label(format!("Offer {name}"))
                // The picker can't be empty, and an empty list offers every model.
                .disabled(on && count == 1)
                .on_toggle(move |s: &mut State| s.toggle_chat_model(&id))
                .build(k);
            compact(k, &name, Some(provider), pick).key(m.id.clone())
        })
        .collect();
    rows.push(note(
        k,
        "The models a chat's picker offers, here and on your phone. A chat keeps its model either way.",
    ));
    rows
}

fn note(k: &Kit, line: &str) -> El {
    row().w_full().px(PAD).py(8.0).child(
        text(line.to_owned())
            .font(k.ty.text_small)
            .color(k.c.text_muted)
            .wrap(),
    )
}

fn caption(k: &Kit, title: &str) -> El {
    text(title.to_owned())
        .font(k.ty.text_small_medium)
        .color(k.c.text_muted)
        .pl(4.0)
        .pt(8.0)
}

fn gui_model(s: &State, page: &SettingsPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let enabled = page
        .settings
        .as_ref()
        .is_some_and(|v| v.browser_use || v.computer_use);
    let current = page.settings.as_ref().and_then(|v| v.gui_model.as_deref());
    let default = s.catalog.default_model().map(|id| s.catalog.label(id));
    let default = default.map_or("Default model".to_owned(), |name| {
        format!("{name} (default)")
    });
    let mut options = vec![ComboboxOption::new(DEFAULT_MODEL, default)];
    if let Some(models) = s.catalog.ready() {
        options.extend(model_options(models));
    }
    let picker = Combobox::new("gui-model", &page.gui_model, &options, |s: &mut State| {
        &mut s
            .settings_page_mut()
            .expect("the picker is on the settings page")
            .gui_model
    })
    .label("Browser and computer use model")
    .current(Some(current.unwrap_or(DEFAULT_MODEL)))
    .search_placeholder("Search models")
    .width(CONTROL - THINKING - GAP)
    .disabled(!enabled)
    .on_choose(|s: &mut State, id, _| {
        let model = (id != DEFAULT_MODEL).then(|| id.to_owned());
        s.change_settings(|v| v.gui_model = model);
    })
    .build(k, layer);
    let pickers = row()
        .gap(GAP)
        .child(picker)
        .child(gui_thinking(s, page, k, layer, enabled));
    with_hint(
        wide(k, "Browser and computer use", pickers),
        k,
        if enabled {
            "Runs browser and computer use tasks. It must read images, and a fast model works best."
        } else {
            "Turn on Browser use or Computer use to choose it."
        },
    )
}

fn gui_thinking(s: &State, page: &SettingsPage, k: &Kit, layer: &mut Vec<El>, enabled: bool) -> El {
    let settings = page.settings.as_ref();
    let model = settings
        .and_then(|v| v.gui_model.as_deref())
        .or_else(|| s.catalog.default_model());
    let levels = match (s.catalog.ready(), model) {
        (Some(models), Some(model)) => levels_of(models, model),
        _ => &[],
    };
    let options: Vec<_> = levels
        .iter()
        .map(|level| ComboboxOption::new(level.clone(), reasoning_name(level)))
        .collect();
    // The extension's default, when the setting is unset.
    let current = settings
        .and_then(|v| v.gui_thinking.as_deref())
        .unwrap_or("medium");
    let current = levels.iter().any(|l| l == current).then_some(current);
    let disabled = !enabled || levels.len() < 2;
    let label = "Browser and computer use reasoning";
    // The default trigger takes the list's width, but the list can't be narrower than
    // Rune's 200-point popover.
    let trigger = Select::new::<&str>("gui-thinking-value", &[])
        .current(current.map(reasoning_name).as_deref())
        .label(label)
        .appearance(SelectAppearance::Field)
        .width(THINKING)
        .disabled(disabled)
        .trigger(k);
    Combobox::new(
        "gui-thinking",
        &page.gui_thinking,
        &options,
        |s: &mut State| {
            &mut s
                .settings_page_mut()
                .expect("the picker is on the settings page")
                .gui_thinking
        },
    )
    .label(label)
    .current(current)
    .trigger(trigger)
    .placement(Placement::BottomEnd)
    .width(200.0)
    .disabled(disabled)
    .on_choose(|s: &mut State, level, _| {
        let level = level.to_owned();
        s.change_settings(|v| v.gui_thinking = Some(level));
    })
    .build(k, layer)
}

fn agent_tools(page: &SettingsPage, k: &Kit) -> Vec<El> {
    let on = |get: fn(&crate::data::Settings) -> bool| page.settings.as_ref().is_some_and(get);
    let mut rows = vec![
        compact(
            k,
            "Tool routing",
            Some(
                "Tells the agent which tool fits each job, and hints when it drifts. It saves turns and tokens.",
            ),
            Switch::new(on(|v| v.tool_routing))
                .hide_label()
                .label("Tool routing")
                .on_toggle(|s: &mut State| s.change_settings(|v| v.tool_routing = !v.tool_routing))
                .build(k),
        ),
        compact(
            k,
            "Second opinion",
            Some(
                "Lets the agent ask the other model family, GPT or Claude, for a read-only review.",
            ),
            Switch::new(on(|v| v.second_opinion))
                .hide_label()
                .label("Second opinion")
                .on_toggle(|s: &mut State| {
                    s.change_settings(|v| v.second_opinion = !v.second_opinion)
                })
                .build(k),
        ),
    ];
    rows.extend(gui_tool(page, k, GuiTool::Browser));
    rows.extend(gui_tool(page, k, GuiTool::Computer));
    if let Some(why) = &page.tools.failed {
        rows.push(error_line(k, format!("Couldn't check the tools: {why}")));
    }
    rows.push(note(
        k,
        "Changes apply to new chats, and to open chats after they idle.",
    ));
    rows
}

fn gui_tool(page: &SettingsPage, k: &Kit, tool: GuiTool) -> Vec<El> {
    let (label, about, install, done) = match tool {
        GuiTool::Browser => (
            "Browser use",
            "Hands tasks in your Chrome to a subagent, through browse.",
            "Install browse from opencode.ai? It then opens Chrome to add its extension.",
            "browse is connected to your browser.",
        ),
        GuiTool::Computer => (
            "Computer use",
            "Hands tasks in Mac apps to a subagent, through Cua Driver.",
            "Install Cua Driver from cua.ai? It then asks for Accessibility and Screen Recording.",
            "Cua Driver is ready.",
        ),
    };
    let tools = &page.tools;
    let on = page.settings.as_ref().is_some_and(|v| v.gui_tool(tool));
    let status = tools.status.map(|all| all.get(tool));
    let hint = match status {
        Some(ToolStatus::Missing) if !on => format!("{about} Turning it on installs it."),
        Some(ToolStatus::NeedsPermissions) => {
            format!("{about} It needs Accessibility and Screen Recording.")
        }
        Some(ToolStatus::Unpaired) => {
            format!(
                "{about} It needs its extension in Chrome. Turning it on opens Chrome to add it."
            )
        }
        _ => about.to_owned(),
    };
    let control = if tools.running == Some(tool) {
        busy(16.0, k)
    } else {
        Switch::new(on)
            .hide_label()
            .label(label)
            // Turning it on may install it, so wait until the status says whether it must.
            .disabled(status.is_none() || tools.running.is_some())
            .on_toggle(move |s: &mut State| s.toggle_gui_tool(tool))
            .build(k)
    };
    let mut rows = vec![compact(k, label, Some(&hint), control)];
    if tools.confirm == Some(tool) {
        let buttons = row()
            .gap(8.0)
            .child(
                Button::new("Cancel")
                    .variant(ButtonVariant::Outline)
                    .on_press(State::cancel_gui_setup)
                    .build(k),
            )
            .child(
                Button::new("Install")
                    .variant(ButtonVariant::Neutral)
                    .on_press(move |s: &mut State| s.set_up_gui_tool(tool))
                    .build(k),
            );
        let question = text(install.to_owned())
            .font(k.ty.ui)
            .color(k.c.text_base)
            .wrap()
            .grow(1.0)
            .min_w(0.0);
        rows.push(status_row(question, buttons));
    }
    match &tools.outcome {
        Some((ended, Outcome::Done)) if *ended == tool => rows.push(
            row().w_full().px(PAD).py(8.0).child(
                text(done.to_owned())
                    .font(k.ty.text_small)
                    .color(k.c.success.fg)
                    .wrap(),
            ),
        ),
        Some((ended, Outcome::Failed { error, log })) if *ended == tool => {
            let why = text(format!("Couldn't set up {}: {error}", label.to_lowercase()))
                .font(k.ty.text_small)
                .color(k.c.danger.fg)
                .wrap()
                .grow(1.0)
                .min_w(0.0);
            let copy = log.clone().map(|log| {
                Button::new("Copy log")
                    .variant(ButtonVariant::Outline)
                    .on_press_tx(move |_: &mut State, tx: &mut Tx| tx.copy(log.clone()))
                    .build(k)
            });
            rows.push(status_row(why, row().children(copy)));
        }
        _ => {}
    }
    rows
}

fn error_line(k: &Kit, why: String) -> El {
    row()
        .w_full()
        .px(PAD)
        .py(8.0)
        .child(text(why).font(k.ty.text_small).color(k.c.danger.fg).wrap())
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
            match &state.link {
                Some(link) => out.push(relay_status(link, k)),
                None => out.push(relay_field(page, pairing.is_none(), k)),
            }
            out.push(compact(
                k,
                &format!("Linked phones ({})", state.phones.len()),
                None,
                Button::new("Link new phone")
                    .variant(ButtonVariant::Neutral)
                    .help("Show a QR code to scan with another phone")
                    .disabled(pairing.is_some())
                    .on_press(State::link_phone)
                    .build(k),
            ));
            if state.phones.is_empty() {
                out.push(
                    row()
                        .w_full()
                        .px(PAD)
                        .py(12.0)
                        .child(text("No phones linked").font(k.ty.ui).color(k.c.text_muted)),
                );
            }
            for id in &state.phones {
                out.push(linked_phone(
                    k,
                    id,
                    page.phone.confirm_unlink.as_deref() == Some(id),
                ));
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

fn relay_status(link: &RelayLink, k: &Kit) -> El {
    let (color, status) = match link.status {
        LinkStatus::Connected => (
            k.c.success.icon,
            format!("Relay connected · {}", host(&link.relay)),
        ),
        LinkStatus::Connecting => (
            k.c.warning.icon,
            format!("Relay connecting · {}", host(&link.relay)),
        ),
        LinkStatus::Rejected => (
            k.c.danger.icon,
            "Relay rejected this Mac. Link a phone again.".to_owned(),
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
    status_row(info, el())
}

fn linked_phone(k: &Kit, id: &str, confirm: bool) -> El {
    let name = format!("Phone {}", id.chars().take(8).collect::<String>());
    let label = if confirm {
        format!("Unlink {name}?")
    } else {
        name
    };
    let info = text(label)
        .font(k.ty.ui)
        .color(k.c.text_base)
        .truncate()
        .grow(1.0)
        .min_w(0.0);
    let phone = id.to_owned();
    let buttons = if confirm {
        row()
            .gap(8.0)
            .child(
                Button::new("Cancel")
                    .variant(ButtonVariant::Outline)
                    .on_press(|s: &mut State| s.confirm_unlink(None))
                    .build(k),
            )
            .child(
                Button::new("Unlink")
                    .variant(ButtonVariant::Danger)
                    .on_press(move |s: &mut State| s.unlink_phone(&phone))
                    .build(k),
            )
    } else {
        row().child(
            Button::new("Unlink")
                .variant(ButtonVariant::Outline)
                .on_press(move |s: &mut State| s.confirm_unlink(Some(phone.clone())))
                .build(k),
        )
    };
    status_row(info, buttons)
        .key(format!("phone-{id}"))
        .help(id)
}

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

fn host(relay: &str) -> &str {
    relay.split_once("://").map_or(relay, |(_, host)| host)
}

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
                .w(LABEL)
                // A larger text size narrows the card, so the label wraps rather than push
                // the control out of it.
                .min_w(0.0)
                .shrink(),
        )
        .child(row().grow(1.0).justify_end().child(control))
}

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
