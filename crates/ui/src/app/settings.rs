use qrcode::QrCode;
use qrcode::render::svg;
use rune::{Image, Shortcut, ShortcutError, TextEdit};
use rune_kit::{ComboboxState, ShortcutRecorderState};
use serde_json::{Value, json};

use super::{Page, State, Window};
use crate::data::{LinkStatus, Pairing, PhoneLink, RelayLink, Settings};
use crate::update;

pub const QR_SIZE: f32 = 184.0;

pub struct SettingsPage {
    pub settings: Option<Settings>,
    pub error: Option<String>,
    pub shortcut: ShortcutRecorderState,
    pub model: ComboboxState,
    pub phone: Phone,
    pub update: Update,
    previous: Option<String>,
}

pub enum Update {
    Checking,
    Latest,
    Available(String),
    Installing(String),
    Failed(String),
}

pub struct Phone {
    pub state: Option<PhoneLink>,
    pub relay: TextEdit,
    pub qr: Option<(String, Image)>,
    pub confirm_unlink: bool,
    pub error: Option<String>,
}

impl Phone {
    pub fn link(&self) -> Option<&RelayLink> {
        self.state.as_ref()?.link.as_ref()
    }

    pub fn pairing(&self) -> Option<&Pairing> {
        self.state.as_ref()?.pairing.as_ref()
    }
}

impl SettingsPage {
    pub fn connecting(&self) -> bool {
        let connecting = self
            .phone
            .link()
            .is_some_and(|l| l.status == LinkStatus::Connecting);
        let pairing = matches!(
            self.phone.pairing(),
            Some(Pairing::Starting | Pairing::Waiting { .. })
        );
        connecting || pairing
    }
}

impl State {
    pub fn settings_page(&self) -> Option<&SettingsPage> {
        match &self.page {
            Page::Settings(page) => Some(page),
            _ => None,
        }
    }

    pub fn settings_page_mut(&mut self) -> Option<&mut SettingsPage> {
        match &mut self.page {
            Page::Settings(page) => Some(page),
            _ => None,
        }
    }

    pub fn open_settings(&mut self) {
        if self.window == Window::Quick {
            self.api.peer().update(|s, _| s.open_settings());
            return self.hide();
        }
        if self.settings_page().is_none() {
            let previous = self.chat().and_then(|p| p.existing()).map(|c| c.id.clone());
            let mut shortcut = ShortcutRecorderState::new(self.registered_shortcut());
            if self.prefs.borrow().shortcut.is_some() && self.hotkey.get().is_none() {
                shortcut.mark_unregistered();
            }
            self.leave_page();
            self.nav += 1;
            self.page = Page::Settings(Box::new(SettingsPage {
                settings: None,
                error: None,
                shortcut,
                model: ComboboxState::default(),
                phone: Phone {
                    state: None,
                    relay: TextEdit::default(),
                    qr: None,
                    confirm_unlink: false,
                    error: None,
                },
                update: Update::Checking,
                previous,
            }));
            self.check_update();
            self.load_settings();
            self.load_phone();
            self.load_catalog();
        }
        self.open();
    }

    pub fn close_settings(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let pairing = page.phone.pairing().is_some();
        let previous = page.previous.take();
        if pairing {
            self.api.call(
                "stop_linking_phone",
                json!({}),
                |_, _: Result<Value, String>, _| {},
            );
        }
        match previous {
            Some(id) => self.open_chat(id),
            None => self.new_chat(),
        }
    }

    pub fn check_update(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        if matches!(page.update, Update::Installing(_)) {
            return;
        }
        page.update = Update::Checking;
        self.api.run(
            |_| async { tokio::task::spawn_blocking(update::available).await },
            |s, result, _| {
                let Some(page) = s.settings_page_mut() else {
                    return;
                };
                page.update = match result.map_err(anyhow::Error::from).and_then(|r| r) {
                    Ok(Some(version)) => Update::Available(version),
                    Ok(None) => Update::Latest,
                    Err(e) => Update::Failed(format!("Couldn't check for updates: {e:#}")),
                };
            },
        );
    }

    pub fn install_update(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let Update::Available(version) = &page.update else {
            return;
        };
        let version = version.clone();
        page.update = Update::Installing(version.clone());
        self.api.run(
            move |_| async move { tokio::task::spawn_blocking(move || update::install(&version)).await },
            |s, result, tx| {
                let done = result
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r)
                    .and_then(|app| update::relaunch(&app));
                match done {
                    Ok(()) => tx.quit(),
                    Err(e) => {
                        if let Some(page) = s.settings_page_mut() {
                            page.update = Update::Failed(format!("Couldn't update: {e:#}"));
                        }
                    }
                }
            },
        );
    }

    fn load_settings(&mut self) {
        self.api.call(
            "get_settings",
            json!({}),
            |s, result: Result<Settings, String>, _| {
                if let Ok(settings) = &result {
                    s.show_expand_tools(settings.expand_tools);
                }
                if let Some(page) = s.settings_page_mut() {
                    match result {
                        Ok(settings) => page.settings = Some(settings),
                        Err(e) => page.error = Some(e),
                    }
                }
            },
        );
    }

    pub fn change_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let Some(settings) = page.settings.as_mut() else {
            return;
        };
        let old = settings.clone();
        change(settings);
        if *settings == old {
            return;
        }
        let expand_tools = settings.expand_tools;
        let args = json!(settings);
        self.show_expand_tools(expand_tools);
        self.api.call(
            "set_settings",
            args,
            move |s, result: Result<Settings, String>, _| {
                let saved = match result {
                    Ok(saved) => saved,
                    Err(e) => {
                        s.fail("Couldn't change the setting", e);
                        old
                    }
                };
                s.show_expand_tools(saved.expand_tools);
                if let Some(page) = s.settings_page_mut() {
                    page.settings = Some(saved);
                }
            },
        );
    }

    pub fn load_phone(&mut self) {
        self.phone_call(
            "get_phone_link",
            json!({}),
            "Couldn't load the phone's link",
        );
    }

    pub fn link_phone(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let relay = page
            .phone
            .link()
            .is_none()
            .then(|| page.phone.relay.text.trim().to_owned());
        self.phone_call(
            "link_phone",
            json!({ "relay": relay }),
            "Couldn't link a phone",
        );
    }

    pub fn stop_linking_phone(&mut self) {
        self.phone_call("stop_linking_phone", json!({}), "Couldn't stop linking");
    }

    pub fn confirm_unlink(&mut self, ask: bool) {
        if let Some(page) = self.settings_page_mut() {
            page.phone.confirm_unlink = ask;
        }
    }

    pub fn unlink_phone(&mut self) {
        self.confirm_unlink(false);
        self.phone_call("unlink_phone", json!({}), "Couldn't unlink the phone");
    }

    fn phone_call(&mut self, tool: &'static str, args: Value, failed: &'static str) {
        self.api.call(
            tool,
            args,
            move |s, result: Result<PhoneLink, String>, _| {
                match result {
                    Ok(state) => s.show_phone(state),
                    Err(e) if tool == "get_phone_link" => {
                        if let Some(page) = s.settings_page_mut() {
                            page.phone.error = Some(e);
                        }
                    }
                    Err(e) => s.fail(failed, e),
                }
                s.sync_tick();
            },
        );
    }

    fn show_phone(&mut self, state: PhoneLink) {
        let Some(page) = self.settings_page() else {
            return;
        };
        let shown = page.phone.qr.as_ref().map(|(url, _)| url.as_str());
        let fresh = match &state.pairing {
            Some(Pairing::Waiting { url }) if shown != Some(url) => self
                .images
                .as_ref()
                .and_then(|images| qr_code(images, url))
                .map(|qr| (url.clone(), qr)),
            _ => None,
        };
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        if page.phone.state.is_none() {
            page.phone.relay = TextEdit::new(state.relay.clone());
        }
        let was_waiting = matches!(page.phone.pairing(), Some(Pairing::Waiting { .. }));
        let linked = state.pairing == Some(Pairing::Linked);
        if fresh.is_some() || !matches!(state.pairing, Some(Pairing::Waiting { .. })) {
            page.phone.qr = fresh;
        }
        page.phone.state = Some(state);
        page.phone.error = None;
        if linked {
            if was_waiting {
                self.notify("Phone linked");
            }
            self.stop_linking_phone();
        }
    }

    pub fn register_saved_shortcut(&mut self) {
        let saved = self.prefs.borrow().shortcut.clone();
        let Some(saved) = saved else { return };
        let registered = saved
            .parse()
            .and_then(|shortcut| self.register_shortcut(shortcut));
        if let Err(e) = registered {
            eprintln!("the shortcut {saved}: {e}");
        }
    }

    pub fn change_shortcut(&mut self, shortcut: Option<&Shortcut>) -> Result<(), ShortcutError> {
        match shortcut {
            Some(shortcut) => self.register_shortcut(shortcut.clone())?,
            None => {
                if let Some(old) = self.hotkey.take() {
                    drop(self.quick().unregister_shortcut(old));
                }
            }
        }
        self.prefs.borrow_mut().shortcut = shortcut.map(Shortcut::to_string);
        self.save_prefs();
        Ok(())
    }

    pub(super) fn load_expand_tools(&mut self) {
        self.api.call(
            "get_settings",
            json!({}),
            |s, result: Result<Settings, String>, _| match result {
                Ok(settings) => s.show_expand_tools(settings.expand_tools),
                Err(e) => eprintln!("get_settings: {e}"),
            },
        );
    }

    fn show_expand_tools(&mut self, on: bool) {
        if self.expand_tools.replace(on) != on {
            self.api.peer().update(|_, _| {});
        }
    }

    fn register_shortcut(&mut self, shortcut: Shortcut) -> Result<(), ShortcutError> {
        let quick = self.quick();
        let id = quick.register_shortcut(shortcut, |s, _| s.toggle_window())?;
        if let Some(old) = self.hotkey.replace(Some(id)) {
            drop(quick.unregister_shortcut(old));
        }
        Ok(())
    }

    fn registered_shortcut(&self) -> Option<Shortcut> {
        self.prefs.borrow().shortcut.as_deref()?.parse().ok()
    }

    fn quick(&self) -> rune::AppHandle<State> {
        match self.window {
            Window::Quick => self.api.app().clone(),
            Window::Main => self.api.peer().clone(),
        }
    }
}

fn qr_code(images: &rune::ImageCache, url: &str) -> Option<Image> {
    let code = QrCode::new(url.as_bytes())
        .inspect_err(|e| eprintln!("making the QR code: {e}"))
        .ok()?;
    let svg = code
        .render::<svg::Color>()
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Some(images.svg(svg.into_bytes(), QR_SIZE, 2.0))
}
