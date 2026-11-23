use rune::{Color, El, Icon, SurfaceFit, Tx, col, el, icon, row, text};
use rune_kit::shell::{Sidebar, SidebarRow};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxOption, ControlSize, Kit, Placement, SegmentedControl,
    Select, SelectAppearance, ShortcutRecorder, Switch, TextInput, shortcut_error_message,
};

use super::widgets::{self, busy, dot, model_options};
use super::{icons, theme};
use crate::app::{Outcome, QR_SIZE, Section, SettingsPage, State, Update};
use crate::data::{
    Account, GuiTool, LinkStatus, Notify, Pairing, PromptKind, RelayLink, ResourceKind,
    ReviewModel, SendBehavior, Settings, SignInPrompt, ToolStatus, levels_of,
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
const GROUP_LIST_VISIBLE: usize = 5;
/// Five single-line rows (44 pt each) plus the four 1 pt dividers between them.
const GROUP_LIST_MAX_HEIGHT: f32 =
    GROUP_LIST_VISIBLE as f32 * 44.0 + (GROUP_LIST_VISIBLE - 1) as f32;
/// The sidecar picker's id for pi's default model, which follows pi's
/// settings; a model's id has a `/`.
const DEFAULT_MODEL: &str = "default";
const NO_REVIEWER: &str = "unset";
/// browse and Cua Driver set up on macOS only, and macOS runs the app and the engine together.
const GUI_TOOLS: bool = cfg!(target_os = "macos");
/// Only Ludolph.app has a login item.
const LOGIN_ITEM: bool = cfg!(target_os = "macos");

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
                    std::iter::once(sidecar(s, page, k, layer))
                        .chain(GUI_TOOLS.then(|| gui_model(s, page, k, layer)))
                        .collect(),
                ))
                .child(caption(k, "Second opinion"))
                .child(group(
                    k,
                    [ReviewFamily::Claude, ReviewFamily::Gpt]
                        .into_iter()
                        .map(|family| review_model(s, page, k, layer, family))
                        .collect(),
                ))
                .child(caption(k, "Chat model picker"))
                .children(chat_models(s, page, k))
        }),
        Section::Accounts => accounts(page, k, layer),
        Section::Resources => resources(page, k),
        Section::Phone => loaded(page, k, || phone(page, k)),
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
    // Keyed by section so each one swaps cleanly and opens scrolled to its top.
    widgets::page(
        col()
            .w_full()
            .h_full()
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(row().w_full().justify_center().child(body)),
        page_key(page.section),
    )
}

/// The settings sections, shown in the main sidebar in place of the chats.
pub fn sidebar(page: &SettingsPage, k: &Kit) -> El {
    let back = SidebarRow::new("Back to chats")
        .icon(&icons::BACK)
        .detail("Esc")
        .on_select(|s: &mut State, _| s.close_settings())
        .build(k)
        .key("back");
    Section::ALL
        .into_iter()
        .fold(
            Sidebar::new().label("Settings").header(back),
            |bar, section| {
                bar.child(
                    SidebarRow::new(title(section))
                        .icon(section_icon(section))
                        .selected(page.section == section)
                        .on_select(move |s: &mut State, _| s.show_settings_section(section))
                        .build(k)
                        .key(key(section)),
                )
            },
        )
        .build(k)
}

fn title(section: Section) -> &'static str {
    match section {
        Section::General => "General",
        Section::Appearance => "Appearance",
        Section::Accounts => "Accounts",
        Section::AgentTools => "Agent tools",
        Section::Model => "Model",
        Section::Resources => "Skills and extensions",
        Section::Phone => "Phone",
        Section::About => "About",
    }
}

fn key(section: Section) -> &'static str {
    match section {
        Section::General => "general",
        Section::Appearance => "appearance",
        Section::Accounts => "accounts",
        Section::AgentTools => "agent-tools",
        Section::Model => "model",
        Section::Resources => "resources",
        Section::Phone => "phone",
        Section::About => "about",
    }
}

fn page_key(section: Section) -> &'static str {
    match section {
        Section::General => "settings-general-page",
        Section::Appearance => "settings-appearance-page",
        Section::Accounts => "settings-accounts-page",
        Section::AgentTools => "settings-agent-tools-page",
        Section::Model => "settings-model-page",
        Section::Resources => "settings-resources-page",
        Section::Phone => "settings-phone-page",
        Section::About => "settings-about-page",
    }
}

fn section_icon(section: Section) -> &'static Icon {
    match section {
        Section::General => &icons::SLIDERS,
        Section::Appearance => &icons::APPEARANCE,
        Section::Accounts => &icons::ACCOUNT,
        Section::AgentTools => &icons::EXTENSIONS,
        Section::Model => &icons::MODELS,
        Section::Resources => &icons::SKILL,
        Section::Phone => &icons::PHONE,
        Section::About => &icons::INFO,
    }
}

/// A section that needs the engine's settings, once they load.
fn loaded(page: &SettingsPage, k: &Kit, content: impl FnOnce() -> El) -> El {
    match (&page.settings, &page.error) {
        (Some(_), _) => content(),
        (None, Some(why)) => failed(k, format!("Couldn't load the settings: {why}")),
        (None, None) => loading(k),
    }
}

fn failed(k: &Kit, why: String) -> El {
    text(why).font(k.ty.ui).color(k.c.text_muted).wrap().pl(4.0)
}

fn loading(k: &Kit) -> El {
    row()
        .w_full()
        .py(24.0)
        .justify_center()
        .child(busy(18.0, k))
}

fn accounts(page: &SettingsPage, k: &'static Kit, layer: &mut Vec<El>) -> El {
    let accounts = &page.accounts;
    let list = match &accounts.list {
        None => return loading(k),
        Some(Err(why)) => return failed(k, format!("Couldn't list pi's providers: {why}")),
        Some(Ok(list)) => list,
    };
    let signed_in: Vec<El> =
        list.iter()
            .filter_map(|a| {
                let source = a.source.as_deref()?;
                // Credentials pi finds outside its sign-ins, which sign-out can't remove.
                let hint = match (source, &a.label) {
                    ("stored", _) => None,
                    (_, Some(label)) => Some(format!("From {label}")),
                    ("models_json_key" | "models_json_command", None) => {
                        Some("From models.json".to_owned())
                    }
                    _ => None,
                };
                let control = if accounts.signing_out.as_ref() == Some(&a.provider) {
                    busy(16.0, k)
                } else if source == "stored" {
                    let provider = a.provider.clone();
                    Button::new("Sign out")
                        .variant(ButtonVariant::Outline)
                        .disabled(accounts.signing_out.is_some())
                        .on_press(move |s: &mut State| s.sign_out(&provider))
                        .build(k)
                } else {
                    el()
                };
                let record = compact(k, &a.name, hint.as_deref(), control);
                let limits = accounts.quota.get(&a.provider).filter(|q| !q.is_empty());
                let record =
                    match limits {
                        None => record,
                        Some(limits) => {
                            let now = chrono::Utc::now();
                            col().w_full().child(record).child(
                                col().w_full().px(PAD).pb(12.0).gap(12.0).children(
                                    limits.iter().map(|w| super::details::limit(w, now, k)),
                                ),
                            )
                        }
                    };
                Some(record.key(a.provider.clone()))
            })
            .collect();
    let mut out = col().w_full().gap(12.0);
    if !signed_in.is_empty() {
        out = out.child(group(k, signed_in));
    }
    out.child(caption(k, "Add a provider"))
        .child(group(k, add_account(page, list, k, layer)))
}

fn add_account(
    page: &SettingsPage,
    list: &[Account],
    k: &'static Kit,
    layer: &mut Vec<El>,
) -> Vec<El> {
    let accounts = &page.accounts;
    let state = &accounts.sign_in;
    let running = state.provider.is_some() && state.result.is_none();
    let options: Vec<ComboboxOption> = list
        .iter()
        .filter(|a| a.source.is_none())
        .map(|a| {
            let mut option = ComboboxOption::new(a.provider.clone(), a.name.clone());
            option.detail.clone_from(&a.provider);
            option
        })
        .collect();
    let picker = Combobox::new("provider", &accounts.picker, &options, |s: &mut State| {
        &mut s
            .settings_page_mut()
            .expect("the picker is on the settings page")
            .accounts
            .picker
    })
    .label("Provider")
    .placeholder("Choose a provider")
    .current(accounts.chosen.as_deref())
    .search_placeholder("Search providers")
    .width(CONTROL)
    .disabled(running)
    .on_choose(|s: &mut State, id, _| s.choose_provider(id))
    .build(k, layer);
    let mut rows = vec![wide(k, "Provider", picker)];
    let name = state.provider.as_deref().map(|p| accounts.name(p));
    if running {
        rows.extend(sign_in(page, k));
    } else if let Some(chosen) = accounts.chosen() {
        let account = chosen.oauth.then(|| {
            Button::new("Sign in")
                .variant(ButtonVariant::Neutral)
                .on_press(|s: &mut State| s.start_sign_in(false))
                .build(k)
        });
        let key = chosen.api_key.then(|| {
            Button::new("Use API key")
                .variant(if chosen.oauth {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::Neutral
                })
                .on_press(|s: &mut State| s.start_sign_in(true))
                .build(k)
        });
        let failed = state.result.as_ref().filter(|r| !r.signed_in).map(|r| {
            let why = r.error.as_deref().unwrap_or("pi didn't say why");
            line(k, format!("Couldn't sign in: {why}"), k.c.danger.fg)
        });
        rows.push(status_row(
            failed.unwrap_or_else(|| el().grow(1.0)),
            row().gap(8.0).children(key).children(account),
        ));
    }
    if let (Some(name), Some(true)) = (name, state.result.as_ref().map(|r| r.signed_in)) {
        rows.push(status_row(
            line(k, format!("Signed in to {name}."), k.c.success.fg),
            el(),
        ));
    }
    rows
}

/// A status line that wraps beside a row's buttons.
fn line(k: &Kit, says: String, color: Color) -> El {
    text(says)
        .font(k.ty.text_small)
        .color(color)
        .wrap()
        .grow(1.0)
        .min_w(0.0)
}

/// The running sign-in's step: pi's question, a device code, or the browser.
fn sign_in(page: &SettingsPage, k: &Kit) -> Vec<El> {
    let accounts = &page.accounts;
    let state = &accounts.sign_in;
    let cancel = || {
        Button::new("Cancel")
            .variant(ButtonVariant::Outline)
            .on_press(State::close_sign_in)
            .build(k)
    };
    let mut rows: Vec<El> = state
        .note
        .iter()
        .map(|note| {
            row()
                .w_full()
                .px(PAD)
                .py(8.0)
                .child(line(k, note.clone(), k.c.text_muted))
        })
        .collect();
    // pi's browser callback usually wins over a pasted code.
    let pasted = |p: &&SignInPrompt| p.kind != PromptKind::ManualCode || accounts.paste;
    if let Some(prompt) = state.prompt.as_ref().filter(pasted) {
        let label = prompt.message.trim_end().trim_end_matches(':');
        if prompt.kind == PromptKind::Select {
            let choices = prompt.options.iter().map(|option| {
                let choice = option.clone();
                Button::new(option.clone())
                    .variant(ButtonVariant::Outline)
                    .on_press(move |s: &mut State| s.answer_sign_in(Some(choice.clone())))
                    .build(k)
            });
            rows.push(wide(k, label, col().gap(8.0).items_end().children(choices)));
            rows.push(status_row(el().grow(1.0), cancel()));
            return rows;
        }
        let mut field = TextInput::new(&accounts.answer)
            .label(label)
            .placeholder(prompt.placeholder.as_deref().unwrap_or(""))
            .width(Some(CONTROL))
            .on_edit(|s: &mut State| {
                &mut s
                    .settings_page_mut()
                    .expect("the field is on the settings page")
                    .accounts
                    .answer
            })
            .on_submit(|s: &mut State, _| s.answer_sign_in(None));
        if prompt.kind != PromptKind::Text {
            field = field.secure();
        }
        rows.push(wide(k, label, field.build(k)));
        let next = Button::new("Continue")
            .variant(ButtonVariant::Neutral)
            .on_press(|s: &mut State| s.answer_sign_in(None))
            .build(k);
        rows.push(status_row(
            el().grow(1.0),
            row().gap(8.0).child(cancel()).child(next),
        ));
        return rows;
    }
    if let Some(device) = &state.device_code {
        let info = col()
            .grow(1.0)
            .min_w(0.0)
            .gap(4.0)
            .child(
                text("Enter this code in your browser")
                    .font(k.ty.ui)
                    .color(k.c.text_base),
            )
            .child(
                text(device.code.clone())
                    .font(k.ty.mono)
                    .color(k.c.text_base)
                    .selectable(true),
            );
        let (code, url) = (device.code.clone(), device.url.clone());
        let buttons = row()
            .gap(8.0)
            .child(cancel())
            .child(
                Button::new("Open page")
                    .variant(ButtonVariant::Outline)
                    .on_press_tx(move |_: &mut State, tx: &mut Tx| drop(tx.open_url(&url)))
                    .build(k),
            )
            .child(
                Button::new("Copy code")
                    .variant(ButtonVariant::Neutral)
                    .on_press_tx(move |_: &mut State, tx: &mut Tx| tx.copy(code.clone()))
                    .build(k),
            );
        rows.push(status_row(info, buttons));
        return rows;
    }
    let (says, paste) = match &state.browser {
        Some(_) => (
            "Finish signing in in your browser.",
            state.prompt.is_some().then(|| {
                Button::new("Paste code")
                    .variant(ButtonVariant::Ghost)
                    .on_press(State::paste_sign_in_code)
                    .build(k)
            }),
        ),
        None => ("Signing in…", None),
    };
    rows.push(status_row(
        line(k, says.to_owned(), k.c.text_base),
        row().gap(8.0).children(paste).child(cancel()),
    ));
    rows
}

fn resources(page: &SettingsPage, k: &Kit) -> El {
    let list = match &page.resources {
        None => return loading(k),
        Some(Err(why)) => {
            return failed(
                k,
                format!("Couldn't list pi's skills and extensions: {why}"),
            );
        }
        Some(Ok(list)) => list,
    };
    let kinds = [
        (ResourceKind::Skills, "Skills", "skills"),
        (ResourceKind::Extensions, "Extensions", "extensions"),
    ];
    let mut out = col().w_full().gap(12.0);
    let mut any = false;
    for (kind, label, kind_id) in kinds {
        let groups = group_by(list.iter().filter(|r| r.kind == kind), |r| r.group.as_str());
        if groups.is_empty() {
            continue;
        }
        any = true;
        out = out.child(caption(k, label));
        for (source, items) in groups {
            let enabled = items.iter().filter(|r| r.enabled).count();
            let rows = items
                .iter()
                .map(|r| {
                    let path = r.path.clone();
                    let pick = Switch::new(r.enabled)
                        .hide_label()
                        .label(format!("Load {}", r.name))
                        .disabled(page.resource_saving)
                        .on_toggle(move |s: &mut State| s.toggle_resource(kind, &path))
                        .build(k);
                    compact(k, &r.name, None, pick).key(r.path.clone())
                })
                .collect();
            out = out.child(grouped_card(
                k,
                GroupCard {
                    key: format!("resource-{kind_id}-{source}"),
                    leading: None,
                    title: source,
                    title_help: Some(source),
                    detail: format!("{enabled} of {}", items.len()),
                    rows,
                    scroll_key: None,
                },
            ));
        }
    }
    if !any {
        return failed(k, "No skills or extensions found.".to_owned());
    }
    out
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
    compact(k, "Accent color", None, choices)
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
    wide(k, "Text size", picker)
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
    let login = LOGIN_ITEM.then(|| {
        compact(
            k,
            "Open at login",
            None,
            Switch::new(at_login)
                .hide_label()
                .label("Open at login")
                .on_toggle(|s: &mut State| {
                    s.change_settings(|v| v.open_at_login = !v.open_at_login)
                })
                .build(k),
        )
    });
    let shortcut = wide(
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
    );
    std::iter::once(shortcut)
        .chain(login)
        .chain([
            tools,
            wide(
                k,
                "Send while working",
                SegmentedControl::new(
                    &["Queue", "Steer"],
                    Some(usize::from(page.settings.as_ref().is_some_and(
                        |settings| settings.send_behavior == SendBehavior::Steer,
                    ))),
                )
                .width(Some(CONTROL))
                .on_pick(|s: &mut State, index, _| {
                    s.change_settings(|settings| {
                        settings.send_behavior = if index == 0 {
                            SendBehavior::Queue
                        } else {
                            SendBehavior::Steer
                        };
                    })
                })
                .build(k),
            ),
        ])
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
        "Names chats and cleans up dictation.",
    )
}

fn chat_models(s: &State, page: &SettingsPage, k: &Kit) -> Vec<El> {
    let Some(models) = s.catalog.ready() else {
        return vec![group(k, vec![loading(k)])];
    };
    if models.is_empty() {
        return vec![failed(k, "No models available.".to_owned())];
    }
    let picked = page
        .settings
        .as_ref()
        .map_or(&[][..], |v| v.chat_models.as_slice());
    let offered = |id: &String| picked.is_empty() || picked.contains(id);
    let total_enabled = models.iter().filter(|m| offered(&m.id)).count();
    group_by(models, |m| m.id.split_once('/').map_or("Other", |(p, _)| p))
        .into_iter()
        .map(|(provider, items)| {
            let enabled = items.iter().filter(|m| offered(&m.id)).count();
            let scroll = (items.len() > GROUP_LIST_VISIBLE)
                .then(|| format!("chat-models-scroll-{provider}"));
            let leading = Some(icon(icons::mark(&items[0].id), 14.0).color(k.c.icon_muted));
            let rows = items
                .iter()
                .map(|m| {
                    let on = offered(&m.id);
                    let name = s.catalog.label(&m.id);
                    let id = m.id.clone();
                    let pick = Switch::new(on)
                        .hide_label()
                        .label(format!("Offer {name}"))
                        // The picker can't be empty, and an empty list offers every model.
                        .disabled(on && total_enabled == 1)
                        .on_toggle(move |s: &mut State| s.toggle_chat_model(&id))
                        .build(k);
                    compact(k, &name, None, pick).key(m.id.clone())
                })
                .collect();
            grouped_card(
                k,
                GroupCard {
                    key: format!("chat-models-{provider}"),
                    leading,
                    title: page.accounts.name(provider),
                    title_help: None,
                    detail: format!("{enabled} of {}", items.len()),
                    rows,
                    scroll_key: scroll,
                },
            )
        })
        .collect()
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
    wide(k, "Browser and computer use", pickers)
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
    let options = reasoning_options(levels);
    // The extension's default, when the setting is unset.
    let current = settings
        .and_then(|v| v.gui_thinking.as_deref())
        .unwrap_or("medium");
    let current = levels.iter().any(|l| l == current).then_some(current);
    let disabled = !enabled || levels.len() < 2;
    let label = "Browser and computer use reasoning";
    let trigger = reasoning_trigger(k, "gui-thinking", label, current, disabled);
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

fn reasoning_options(levels: &[String]) -> Vec<ComboboxOption> {
    levels
        .iter()
        .map(|level| ComboboxOption::new(level.clone(), reasoning_name(level)))
        .collect()
}

fn reasoning_trigger(k: &Kit, key: &str, label: &str, current: Option<&str>, disabled: bool) -> El {
    // Keep the field narrow without shrinking Rune's 200-point popover.
    Select::new::<&str>(format!("{key}-value"), &[])
        .current(current.map(reasoning_name).as_deref())
        .label(label)
        .appearance(SelectAppearance::Field)
        .width(THINKING)
        .disabled(disabled)
        .trigger(k)
}

#[derive(Clone, Copy)]
enum ReviewFamily {
    Claude,
    Gpt,
}

impl ReviewFamily {
    fn matches(self, id: &str) -> bool {
        let prefix = match self {
            Self::Claude => "claude-",
            Self::Gpt => "gpt-",
        };
        id.rsplit_once('/')
            .is_some_and(|(_, model)| model.starts_with(prefix))
    }

    fn choice(self, settings: &Settings) -> &Option<ReviewModel> {
        match self {
            Self::Claude => &settings.second_opinion_claude,
            Self::Gpt => &settings.second_opinion_gpt,
        }
    }

    fn choice_mut(self, settings: &mut Settings) -> &mut Option<ReviewModel> {
        match self {
            Self::Claude => &mut settings.second_opinion_claude,
            Self::Gpt => &mut settings.second_opinion_gpt,
        }
    }
}

fn review_model(
    s: &State,
    page: &SettingsPage,
    k: &Kit,
    layer: &mut Vec<El>,
    family: ReviewFamily,
) -> El {
    let (label, key, controls) = match family {
        ReviewFamily::Claude => ("Claude reviewer", "review-claude", &page.claude_review),
        ReviewFamily::Gpt => ("GPT reviewer", "review-gpt", &page.gpt_review),
    };
    let controls_mut: fn(&mut SettingsPage) -> &mut _ = match family {
        ReviewFamily::Claude => |p| &mut p.claude_review,
        ReviewFamily::Gpt => |p| &mut p.gpt_review,
    };
    let choice = page
        .settings
        .as_ref()
        .and_then(|v| family.choice(v).as_ref());
    let models = s.catalog.ready().unwrap_or_default();
    let mut options = vec![ComboboxOption::new(NO_REVIEWER, "Not set")];
    options.extend(model_options(
        models
            .iter()
            .filter(|m| family.matches(&m.id) && !m.reasoning_levels.is_empty()),
    ));
    let placeholder = choice.map_or_else(String::new, |peer| {
        if s.catalog.ready().is_some() {
            format!("{} (unavailable)", peer.model)
        } else {
            peer.model.clone()
        }
    });
    let picker = Combobox::new(key, &controls.model, &options, move |s: &mut State| {
        &mut controls_mut(
            s.settings_page_mut()
                .expect("the picker is on the settings page"),
        )
        .model
    })
    .label(label)
    .current(Some(choice.map_or(NO_REVIEWER, |peer| peer.model.as_str())))
    .placeholder(placeholder)
    .search_placeholder("Search models")
    .width(CONTROL - THINKING - GAP)
    .on_choose(move |s: &mut State, id, _| {
        let choice = if id == NO_REVIEWER {
            None
        } else {
            let levels = levels_of(s.catalog.ready().unwrap_or_default(), id);
            let Some(thinking) = levels
                .iter()
                .find(|level| level.as_str() == "medium")
                .or_else(|| levels.first())
            else {
                return;
            };
            Some(ReviewModel {
                model: id.to_owned(),
                thinking: thinking.clone(),
            })
        };
        s.change_settings(|v| *family.choice_mut(v) = choice);
    })
    .build(k, layer);

    let levels = choice.map_or(&[][..], |peer| levels_of(models, &peer.model));
    let current = choice
        .map(|peer| peer.thinking.as_str())
        .filter(|level| levels.iter().any(|supported| supported == level));
    let disabled = levels.is_empty() || (levels.len() == 1 && current.is_some());
    let thinking_label = format!("{label} reasoning");
    let thinking_key = format!("{key}-thinking");
    let trigger = reasoning_trigger(k, &thinking_key, &thinking_label, current, disabled);
    let thinking = Combobox::new(
        thinking_key,
        &controls.thinking,
        &reasoning_options(levels),
        move |s: &mut State| {
            &mut controls_mut(
                s.settings_page_mut()
                    .expect("the picker is on the settings page"),
            )
            .thinking
        },
    )
    .label(thinking_label)
    .current(current)
    .trigger(trigger)
    .placement(Placement::BottomEnd)
    .width(200.0)
    .disabled(disabled)
    .on_choose(move |s: &mut State, level, _| {
        s.change_settings(|v| {
            if let Some(peer) = family.choice_mut(v) {
                peer.thinking = level.to_owned();
            }
        });
    })
    .build(k, layer);
    wide(k, label, row().gap(GAP).child(picker).child(thinking))
}

fn agent_tools(page: &SettingsPage, k: &Kit) -> Vec<El> {
    let on = |get: fn(&crate::data::Settings) -> bool| page.settings.as_ref().is_some_and(get);
    let mut rows = vec![
        compact(
            k,
            "Tool routing",
            Some("Points the agent to the right tool for each job."),
            Switch::new(on(|v| v.tool_routing))
                .hide_label()
                .label("Tool routing")
                .on_toggle(|s: &mut State| s.change_settings(|v| v.tool_routing = !v.tool_routing))
                .build(k),
        ),
        compact(
            k,
            "Second opinion",
            Some("Choose GPT and Claude reviewers in Model settings."),
            Switch::new(on(|v| v.second_opinion))
                .hide_label()
                .label("Second opinion")
                .on_toggle(|s: &mut State| {
                    s.change_settings(|v| v.second_opinion = !v.second_opinion)
                })
                .build(k),
        ),
    ];
    if GUI_TOOLS {
        rows.extend(gui_tool(page, k, GuiTool::Browser));
        rows.extend(gui_tool(page, k, GuiTool::Computer));
    }
    if let Some(why) = &page.tools.failed {
        rows.push(error_line(k, format!("Couldn't check the tools: {why}")));
    }
    rows
}

fn gui_tool(page: &SettingsPage, k: &Kit, tool: GuiTool) -> Vec<El> {
    let (label, about, install, done) = match tool {
        GuiTool::Browser => (
            "Browser use",
            "Lets the agent use Chrome.",
            "Install browse from opencode.ai? It then opens Chrome to add its extension.",
            "browse is connected to your browser.",
        ),
        GuiTool::Computer => (
            "Computer use",
            "Lets the agent use Mac apps.",
            "Install Cua Driver from cua.ai? It then asks for Accessibility and Screen Recording.",
            "Cua Driver is ready.",
        ),
    };
    let tools = &page.tools;
    let on = page.settings.as_ref().is_some_and(|v| v.gui_tool(tool));
    let status = tools.status.map(|all| all.get(tool));
    let hint = match status {
        Some(ToolStatus::NeedsPermissions) => "Needs Accessibility and Screen Recording.",
        Some(ToolStatus::Unpaired) => "Needs its Chrome extension.",
        _ => about,
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
    let mut rows = vec![compact(k, label, Some(hint), control)];
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

fn phone(page: &SettingsPage, k: &Kit) -> El {
    let notify = page.settings.as_ref().map_or(Notify::Away, |v| v.notify);
    let notify = wide(
        k,
        "Send notifications",
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
    let mut out = col().w_full().gap(12.0);
    match &page.phone.state {
        None => {
            out = out.child(group(k, vec![loading(k)]));
            if let Some(why) = &page.phone.error {
                out = out.child(error_line(k, why.clone()));
            }
        }
        Some(state) => {
            let pairing = state.pairing.as_ref();
            let connection = match &state.link {
                Some(link) => relay_status(link, k),
                None => relay_field(page, pairing.is_none(), k),
            };
            out = out.child(group(k, vec![connection]));
            if let Some(why) = &page.phone.error {
                out = out.child(error_line(k, why.clone()));
            }
            let mut header = row().w_full().center_y().pl(4.0).pt(8.0).gap(6.0).child(
                text("Linked phones")
                    .font(k.ty.text_small_medium)
                    .color(k.c.text_muted),
            );
            if !state.phones.is_empty() {
                header = header
                    .child(
                        text(state.phones.len().to_string())
                            .font(k.ty.text_small)
                            .color(k.c.text_faint),
                    )
                    .child(rune::spacer())
                    .child(
                        Button::new("Link phone")
                            .icon(&icons::PLUS)
                            .variant(ButtonVariant::Ghost)
                            .control_size(ControlSize::Small)
                            .help("Show a QR code to scan with another phone")
                            .disabled(pairing.is_some())
                            .on_press(State::link_phone)
                            .build(k),
                    );
            }
            out = out.child(header);
            if !state.phones.is_empty() {
                let phones = state
                    .phones
                    .iter()
                    .map(|id| linked_phone(k, id, page.phone.confirm_unlink.as_deref() == Some(id)))
                    .collect();
                out = out.child(group(k, phones));
            } else if pairing.is_none() {
                let add = Button::new("Link phone")
                    .icon(&icons::PLUS)
                    .variant(ButtonVariant::Contrast)
                    .help("Show a QR code to scan with your phone")
                    .on_press(State::link_phone)
                    .build(k);
                out = out.child(group(
                    k,
                    vec![compact(
                        k,
                        "No phones linked",
                        Some("Scan a QR code to follow chats from your phone."),
                        add,
                    )],
                ));
            }
            if let Some(pairing) = pairing {
                out = out.child(group(k, vec![pairing_block(page, pairing, k)]));
            }
        }
    }
    out.child(caption(k, "Notifications"))
        .child(group(k, vec![notify]))
}

fn relay_status(link: &RelayLink, k: &Kit) -> El {
    let (color, status, detail) = match link.status {
        LinkStatus::Connected => (k.c.success.icon, "Relay connected", None),
        LinkStatus::Connecting => (k.c.warning.icon, "Relay connecting…", None),
        LinkStatus::Rejected => (
            k.c.danger.icon,
            "Relay rejected this Mac",
            Some("Link a phone again."),
        ),
    };
    let mut info = col().grow(1.0).min_w(0.0).gap(2.0).child(
        row()
            .gap(8.0)
            .center_y()
            .child(dot(color))
            .child(text(status).font(k.ty.ui).color(k.c.text_base).truncate()),
    );
    if let Some(why) = link.error.as_deref().or(detail) {
        info = info.child(
            text(why.to_owned())
                .font(k.ty.text_small)
                .color(k.c.text_muted)
                .wrap(),
        );
    }
    let address = host(&link.relay);
    let trailing = if address.is_empty() {
        el()
    } else {
        text(address.to_owned())
            .font(k.ty.text_small)
            .color(k.c.text_muted)
            .truncate()
    };
    status_row(info, trailing)
}

fn linked_phone(k: &Kit, id: &str, confirm: bool) -> El {
    let name = format!("Phone {}", id.chars().take(8).collect::<String>());
    let label = if confirm {
        format!("Unlink {name}?")
    } else {
        name
    };
    let badge = el()
        .size(28.0, 28.0)
        .radius(7.0)
        .fill(k.c.layer_02)
        .center()
        .child(icon(&icons::PHONE, 14.0).color(k.c.icon_muted))
        .a11y_hidden();
    let info = row()
        .grow(1.0)
        .min_w(0.0)
        .center_y()
        .gap(10.0)
        .child(badge)
        .child(
            text(label)
                .font(k.ty.ui)
                .color(k.c.text_base)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        );
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
    wide(k, "Relay", input)
}

fn pairing_block(page: &SettingsPage, pairing: &Pairing, k: &Kit) -> El {
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Outline)
        .on_press(State::stop_linking_phone)
        .build(k);
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
                .child(
                    text("Scan with your phone's camera")
                        .font(k.ty.ui)
                        .color(k.c.text_base),
                )
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
        Pairing::Starting | Pairing::Linked => col().items_center().gap(10.0).child(busy(18.0, k)),
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
                .on_press_tx(State::install_update)
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

struct GroupCard<'a> {
    key: String,
    leading: Option<El>,
    title: &'a str,
    title_help: Option<&'a str>,
    detail: String,
    rows: Vec<El>,
    scroll_key: Option<String>,
}

fn group_by<'a, T>(
    items: impl IntoIterator<Item = &'a T>,
    key: impl Fn(&'a T) -> &'a str,
) -> Vec<(&'a str, Vec<&'a T>)> {
    let mut groups: Vec<(&'a str, Vec<&'a T>)> = Vec::new();
    for item in items {
        let name = key(item);
        if let Some((_, list)) = groups.iter_mut().find(|(existing, _)| *existing == name) {
            list.push(item);
        } else {
            groups.push((name, vec![item]));
        }
    }
    groups
}

fn divided(k: &Kit, rows: Vec<El>) -> El {
    let mut list = col().w_full();
    for (i, line) in rows.into_iter().enumerate() {
        if i > 0 {
            list = list.child(el().w_full().h(1.0).fill(k.c.border_muted));
        }
        list = list.child(line);
    }
    list
}

fn grouped_card(k: &Kit, card: GroupCard<'_>) -> El {
    let mut header = row()
        .w_full()
        .px(PAD)
        .py(10.0)
        .center_y()
        .gap(8.0)
        .children(card.leading)
        .child(
            text(card.title.to_owned())
                .font(k.ty.ui_medium)
                .color(k.c.text_base)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            text(card.detail)
                .font(k.ty.text_small)
                .color(k.c.text_muted),
        );
    if let Some(help) = card.title_help {
        header = header.help(help);
    }
    let mut list = divided(k, card.rows);
    if let Some(scroll_key) = card.scroll_key {
        list = list
            .key(scroll_key)
            .max_h(GROUP_LIST_MAX_HEIGHT)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            });
    }
    col()
        .key(card.key)
        .w_full()
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_muted)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_muted))
        .child(list)
}

fn group(k: &Kit, rows: Vec<El>) -> El {
    divided(k, rows)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_muted)
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
