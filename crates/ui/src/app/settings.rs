//! The settings page, in the main window: the global shortcut, which the
//! app registers itself, the engine's settings, and the phone's link.

use qrcode::QrCode;
use qrcode::render::svg;
use rune::{Image, Shortcut, ShortcutError, TextEdit};
use rune_kit::{ComboboxState, ShortcutRecorderState};
use serde_json::{Value, json};

use super::{Page, State, Window};
use crate::data::{LinkStatus, Pairing, PhoneLink, RelayLink, Settings};

/// The QR code's side in points.
pub const QR_SIZE: f32 = 184.0;

/// The settings page's own state.
pub struct SettingsPage {
    /// The engine's settings, once loaded.
    pub settings: Option<Settings>,
    /// Why they couldn't be loaded.
    pub error: Option<String>,
    pub shortcut: ShortcutRecorderState,
    /// The sidecar model's picker.
    pub model: ComboboxState,
    pub phone: Phone,
    /// The chat shown before, which going back opens again.
    previous: Option<String>,
}

/// The phone's link, the relay field, and the QR code a phone scans.
pub struct Phone {
    /// `None` until loaded.
    pub state: Option<PhoneLink>,
    /// The relay to link through, until the Mac is linked.
    pub relay: TextEdit,
    /// The QR code of the page a waiting pairing opens, and that page.
    pub qr: Option<(String, Image)>,
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
    /// Whether the phone's link is trying to connect, or a phone is
    /// linking, so the state is worth checking again.
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

    /// Open the settings in the main window, from either window.
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
                    error: None,
                },
                previous,
            }));
            self.load_settings();
            self.load_phone();
            self.load_catalog();
        }
        self.open();
    }

    /// Leave the settings for the chat shown before them. A QR code shown
    /// stops working.
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

    fn load_settings(&mut self) {
        self.api.call(
            "get_settings",
            json!({}),
            |s, result: Result<Settings, String>, _| {
                if let Some(page) = s.settings_page_mut() {
                    match result {
                        Ok(settings) => page.settings = Some(settings),
                        Err(e) => page.error = Some(e),
                    }
                }
            },
        );
    }

    /// Change the engine's settings with `change`, showing the change at
    /// once and undoing it if the engine refuses it.
    pub fn change_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let Some(old) = page.settings.clone() else {
            return;
        };
        let mut new = old.clone();
        change(&mut new);
        if new == old {
            return;
        }
        page.settings = Some(new.clone());
        let args = serde_json::to_value(&new).unwrap_or(Value::Null);
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
                if let Some(page) = s.settings_page_mut() {
                    page.settings = Some(saved);
                }
            },
        );
    }

    /// Load how the phone's link stands.
    pub fn load_phone(&mut self) {
        self.phone_call(
            "get_phone_link",
            json!({}),
            "Couldn't load the phone's link",
        );
    }

    /// Show a QR code that links a phone through the relay in the field,
    /// or the relay this Mac is linked to.
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

    /// Stop showing the QR code; it stops working.
    pub fn stop_linking_phone(&mut self) {
        self.phone_call("stop_linking_phone", json!({}), "Couldn't stop linking");
    }

    pub fn unlink_phone(&mut self) {
        self.phone_call("unlink_phone", json!({}), "Couldn't unlink the phone");
    }

    /// Call one of the engine's phone link tools, which all answer with the
    /// link's state, and show it.
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

    /// Show the phone link's `state`: draw a new pairing's QR code, and
    /// say when a phone has linked.
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
            // How it ended matters only once.
            self.stop_linking_phone();
        }
    }

    /// Register the saved global shortcut, as the app starts. One taken by
    /// another app is left for the settings to show.
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

    /// Use `shortcut` from now on in place of the current one, or none. The
    /// current one stays if the new one can't be registered.
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

    /// Open a chat's tool groups from the start if `on`, else keep them
    /// closed until clicked.
    pub fn expand_tools(&mut self, on: bool) {
        self.prefs.borrow_mut().expand_tools = on;
        self.save_prefs();
    }

    /// Register `shortcut` for the quick window, then let the old one go.
    fn register_shortcut(&mut self, shortcut: Shortcut) -> Result<(), ShortcutError> {
        let quick = self.quick();
        let id = quick.register_shortcut(shortcut, |s, _| s.toggle_window())?;
        if let Some(old) = self.hotkey.replace(Some(id)) {
            drop(quick.unregister_shortcut(old));
        }
        Ok(())
    }

    /// The saved shortcut, which is also the registered one unless another
    /// app holds it.
    fn registered_shortcut(&self) -> Option<Shortcut> {
        self.prefs.borrow().shortcut.as_deref()?.parse().ok()
    }

    /// The quick window's handle, which owns the shortcut.
    fn quick(&self) -> rune::AppHandle<State> {
        match self.window {
            Window::Quick => self.api.app().clone(),
            Window::Main => self.api.peer().clone(),
        }
    }
}

/// `url` as a QR code, dark on white like a printed one, which every
/// camera reads.
fn qr_code(images: &rune::ImageCache, url: &str) -> Option<Image> {
    let code = QrCode::new(url.as_bytes())
        .inspect_err(|e| eprintln!("making the QR code: {e}"))
        .ok()?;
    let svg = code
        .render::<svg::Color>()
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    // At 2x, the size Retina screens draw it at.
    Some(images.svg(svg.into_bytes(), QR_SIZE, 2.0))
}
