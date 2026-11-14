//! The settings page, in the main window: the global shortcut, which the
//! app registers itself, the engine's settings, and the phone's link.

use rune::{Shortcut, ShortcutError, TextEdit};
use rune_kit::{ComboboxState, ShortcutRecorderState};
use serde_json::{Value, json};

use super::{Page, State, Window};
use crate::data::{LinkStatus, PhoneLink, Settings};

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

/// The phone's link, and the field to paste a new one's code in.
pub struct Phone {
    /// `None` until loaded, then the link, if the Mac has one.
    pub link: Option<Option<PhoneLink>>,
    pub code: TextEdit,
    pub linking: bool,
    pub error: Option<String>,
}

impl SettingsPage {
    /// Whether the phone's link is trying to connect, so its status is
    /// worth checking again.
    pub fn connecting(&self) -> bool {
        matches!(
            &self.phone.link,
            Some(Some(PhoneLink {
                status: LinkStatus::Connecting,
                ..
            }))
        )
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
                    link: None,
                    code: TextEdit::default(),
                    linking: false,
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

    /// Leave the settings for the chat shown before them.
    pub fn close_settings(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        match page.previous.take() {
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
        self.api.call(
            "get_phone_link",
            json!({}),
            |s, result: Result<Option<PhoneLink>, String>, _| {
                if let Some(page) = s.settings_page_mut() {
                    match result {
                        Ok(link) => page.phone.link = Some(link),
                        Err(e) => page.phone.error = Some(e),
                    }
                }
                s.sync_tick();
            },
        );
    }

    /// Link the Mac with the code pasted in the phone's field.
    pub fn link_phone(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let code = page.phone.code.text.trim().to_owned();
        if code.is_empty() || page.phone.linking {
            return;
        }
        page.phone.linking = true;
        page.phone.error = None;
        self.api.call(
            "link_phone",
            json!({ "code": code }),
            |s, result: Result<PhoneLink, String>, _| {
                if let Some(page) = s.settings_page_mut() {
                    page.phone.linking = false;
                    match result {
                        Ok(link) => {
                            page.phone.link = Some(Some(link));
                            page.phone.code = TextEdit::default();
                        }
                        Err(e) => page.phone.error = Some(e),
                    }
                }
                s.sync_tick();
            },
        );
    }

    pub fn unlink_phone(&mut self) {
        self.api.call(
            "unlink_phone",
            json!({}),
            |s, result: Result<Value, String>, _| {
                if let Err(e) = result {
                    return s.fail("Couldn't unlink the phone", e);
                }
                if let Some(page) = s.settings_page_mut() {
                    page.phone.link = Some(None);
                }
            },
        );
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
