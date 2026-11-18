use chrono::{TimeDelta, Utc};
use qrcode::QrCode;
use qrcode::render::svg;
use rune::{Image, Shortcut, ShortcutError, TextEdit, Tx};
use rune_kit::{ComboboxState, ShortcutRecorderState};
use serde_json::{Value, json};

use super::{Page, State, Window};
use crate::client::Subscription;
use crate::data::{
    Account, GuiTool, LinkStatus, Pairing, PhoneLink, PromptKind, RelayLink, Resource,
    ResourceKind, Settings, Setup, SignIn, ToolStatus, ToolStatuses,
};
use crate::update;

pub const QR_SIZE: f32 = 184.0;
const RELEASE_CHECK_EVERY: TimeDelta = TimeDelta::days(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    General,
    Appearance,
    Accounts,
    AgentTools,
    Model,
    Resources,
    Phone,
    About,
}

impl Section {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::Appearance,
        Self::Accounts,
        Self::AgentTools,
        Self::Model,
        Self::Resources,
        Self::Phone,
        Self::About,
    ];
}

pub struct SettingsPage {
    pub section: Section,
    pub settings: Option<Settings>,
    pub error: Option<String>,
    pub shortcut: ShortcutRecorderState,
    pub model: ComboboxState,
    pub gui_model: ComboboxState,
    pub gui_thinking: ComboboxState,
    pub claude_review: ReviewPickers,
    pub gpt_review: ReviewPickers,
    pub tools: Tools,
    /// Listing them starts a pi, so they load when their section first shows.
    pub resources: Option<Result<Vec<Resource>, String>>,
    /// A switch is saving; the next one waits, since each rewrites pi's settings.
    pub resource_saving: bool,
    pub accounts: Accounts,
    pub phone: Phone,
    pub update: Update,
    previous: Option<String>,
}

#[derive(Default)]
pub struct ReviewPickers {
    pub model: ComboboxState,
    pub thinking: ComboboxState,
}

/// pi's providers and their sign-in. Listing them starts a pi, so they load when their section
/// first shows.
#[derive(Default)]
pub struct Accounts {
    pub list: Option<Result<Vec<Account>, String>>,
    pub picker: ComboboxState,
    pub chosen: Option<String>,
    pub sign_in: SignIn,
    pub answer: TextEdit,
    pub signing_out: Option<String>,
    /// The engine's sign-in this page shows; other states are late replies about earlier ones.
    attempt: Option<u64>,
    /// The start sent last, while its attempt isn't known yet. A reply for another start is late.
    starting: Option<u64>,
    starts: u64,
    /// The user pastes the browser's code, since its page couldn't reach Ludolph.
    pub paste: bool,
    feed: Option<Subscription>,
}

impl Accounts {
    pub fn chosen(&self) -> Option<&Account> {
        let list = self.list.as_ref()?.as_ref().ok()?;
        list.iter()
            .find(|a| Some(&a.provider) == self.chosen.as_ref())
    }

    pub fn name<'a>(&'a self, provider: &'a str) -> &'a str {
        let list = self.list.as_ref().and_then(|l| l.as_ref().ok());
        list.and_then(|l| l.iter().find(|a| a.provider == provider))
            .map_or(provider, |a| a.name.as_str())
    }
}

/// Browser and computer use need command-line tools and macOS permissions before they can run.
#[derive(Default)]
pub struct Tools {
    pub status: Option<ToolStatuses>,
    pub failed: Option<String>,
    /// The tool whose install waits for the user's go-ahead.
    pub confirm: Option<GuiTool>,
    pub running: Option<GuiTool>,
    /// How the last setup ended, shown under its tool until the next toggle.
    pub outcome: Option<(GuiTool, Outcome)>,
}

pub enum Outcome {
    Done,
    Failed { error: String, log: Option<String> },
}

pub enum Update {
    Checking,
    Latest,
    Available(String),
    #[cfg_attr(
        target_os = "linux",
        expect(dead_code, reason = "Linux opens the release page for updates")
    )]
    Installing(String),
    Failed(String),
}

pub struct Phone {
    pub state: Option<PhoneLink>,
    pub relay: TextEdit,
    pub qr: Option<(String, Image)>,
    pub confirm_unlink: Option<String>,
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
                section: Section::default(),
                settings: None,
                error: None,
                shortcut,
                model: ComboboxState::default(),
                gui_model: ComboboxState::default(),
                gui_thinking: ComboboxState::default(),
                claude_review: ReviewPickers::default(),
                gpt_review: ReviewPickers::default(),
                tools: Tools::default(),
                resources: None,
                resource_saving: false,
                accounts: Accounts::default(),
                phone: Phone {
                    state: None,
                    relay: TextEdit::default(),
                    qr: None,
                    confirm_unlink: None,
                    error: None,
                },
                update: Update::Checking,
                previous,
            }));
            self.check_update();
            self.load_settings();
            self.load_tools();
            self.load_phone();
            self.load_catalog();
        }
        self.open();
    }

    pub fn show_settings_section(&mut self, section: Section) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        page.section = section;
        let (resources, accounts) = (page.resources.is_none(), page.accounts.list.is_none());
        if section == Section::Resources && resources {
            self.resources_call("list_resources", json!({}));
        }
        if section == Section::Accounts && accounts {
            self.load_accounts();
        }
    }

    fn load_accounts(&mut self) {
        self.api.call(
            "list_accounts",
            json!({}),
            |s, result: Result<Vec<Account>, String>, _| s.show_accounts(result),
        );
    }

    fn show_accounts(&mut self, result: Result<Vec<Account>, String>) {
        if let Some(page) = self.settings_page_mut() {
            let accounts = &mut page.accounts;
            accounts.signing_out = None;
            if let Ok(list) = &result
                && list
                    .iter()
                    .any(|a| Some(&a.provider) == accounts.chosen.as_ref() && a.source.is_some())
            {
                accounts.chosen = None;
            }
            accounts.list = Some(result);
        }
    }

    pub fn choose_provider(&mut self, provider: &str) {
        self.close_sign_in();
        if let Some(page) = self.settings_page_mut() {
            page.accounts.chosen = Some(provider.to_owned());
        }
    }

    pub fn start_sign_in(&mut self, api_key: bool) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let Some(provider) = page.accounts.chosen.clone() else {
            return;
        };
        let accounts = &mut page.accounts;
        accounts.answer = TextEdit::default();
        accounts.paste = false;
        accounts.starts += 1;
        let start = accounts.starts;
        accounts.starting = Some(start);
        accounts.attempt = None;
        accounts.sign_in = SignIn {
            provider: Some(provider.clone()),
            ..SignIn::default()
        };
        if accounts.feed.is_none() {
            let feed = self
                .api
                .subscribe("sign_in", json!({}), |s, _, _| s.refresh_sign_in());
            if let Some(page) = self.settings_page_mut() {
                page.accounts.feed = Some(feed);
            }
        }
        self.api.call(
            "sign_in",
            json!({"provider": provider, "apiKey": api_key}),
            move |s, result: Result<SignIn, String>, tx| s.started_sign_in(start, result, tx),
        );
    }

    fn started_sign_in(&mut self, start: u64, result: Result<SignIn, String>, tx: &mut Tx) {
        let page = self.settings_page_mut();
        let wanted = page.is_some_and(|p| p.accounts.starting == Some(start));
        match result {
            // Cancelling names the attempt, so it does nothing once a newer start replaced it.
            Ok(state) if !wanted => self.cancel_sign_in(state.attempt),
            Ok(state) => {
                if let Some(page) = self.settings_page_mut() {
                    page.accounts.starting = None;
                    page.accounts.attempt = Some(state.attempt);
                }
                self.show_sign_in(state, tx);
                // Updates sent before the attempt was known were dropped.
                self.refresh_sign_in();
            }
            Err(e) => {
                if let Some(page) = self.settings_page_mut().filter(|_| wanted) {
                    page.accounts.starting = None;
                    page.accounts.sign_in = SignIn::default();
                }
                self.fail("Couldn't start signing in", e);
            }
        }
    }

    fn refresh_sign_in(&mut self) {
        self.api.call(
            "sign_in_state",
            json!({}),
            |s, result: Result<SignIn, String>, tx| match result {
                Ok(state) => s.show_sign_in(state, tx),
                Err(e) => eprintln!("sign_in_state: {e}"),
            },
        );
    }

    fn show_sign_in(&mut self, state: SignIn, tx: &mut Tx) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        if page.accounts.attempt != Some(state.attempt) {
            return;
        }
        let accounts = &mut page.accounts;
        let signed_in =
            accounts.sign_in.result.is_none() && state.result.as_ref().is_some_and(|r| r.signed_in);
        if accounts.sign_in.prompt.as_ref().map(|p| &p.id) != state.prompt.as_ref().map(|p| &p.id) {
            accounts.answer = TextEdit::default();
        }
        // Each page opens once, when the engine first reports it.
        let browser = state
            .browser
            .as_ref()
            .filter(|url| accounts.sign_in.browser.as_ref() != Some(url));
        let device_page = state
            .device_code
            .as_ref()
            .filter(|code| code.open && accounts.sign_in.device_code.as_ref() != Some(code))
            .map(|code| &code.url);
        for url in browser.into_iter().chain(device_page) {
            if let Err(e) = tx.open_url(url) {
                eprintln!("opening a sign-in page: {e}");
            }
        }
        accounts.sign_in = state;
        if signed_in {
            accounts.chosen = None;
            self.load_accounts();
            // The user approved it in the browser, so Ludolph comes back to show it worked.
            self.api.app().focus();
        }
    }

    /// Answers pi's question with `choice`, or with what the field holds.
    pub fn answer_sign_in(&mut self, choice: Option<String>) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let Some(prompt) = &page.accounts.sign_in.prompt else {
            return;
        };
        let id = prompt.id.clone();
        // A text question may take a blank answer, such as GitHub's "blank for github.com".
        let blank_ok = prompt.kind == PromptKind::Text;
        let value = match choice {
            Some(choice) => choice,
            None => std::mem::take(&mut page.accounts.answer).text.clone(),
        };
        if value.trim().is_empty() && !blank_ok {
            return;
        }
        self.api.call(
            "answer_sign_in",
            json!({"prompt": id, "value": value}),
            |s, result: Result<SignIn, String>, tx| match result {
                Ok(state) => s.show_sign_in(state, tx),
                Err(e) => s.fail("Couldn't sign in", e),
            },
        );
    }

    pub fn paste_sign_in_code(&mut self) {
        if let Some(page) = self.settings_page_mut() {
            page.accounts.paste = true;
        }
    }

    /// Stops a running sign-in, or clears how the last one ended.
    pub fn close_sign_in(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let accounts = &mut page.accounts;
        let attempt = accounts.attempt.take();
        accounts.starting = None;
        accounts.paste = false;
        accounts.sign_in = SignIn::default();
        accounts.feed = None;
        // A start still on its way is cancelled when its reply comes.
        if let Some(attempt) = attempt {
            self.cancel_sign_in(attempt);
        }
    }

    fn cancel_sign_in(&mut self, attempt: u64) {
        self.api.call(
            "cancel_sign_in",
            json!({"attempt": attempt}),
            |_, _: Result<Value, String>, _| {},
        );
    }

    pub fn sign_out(&mut self, provider: &str) {
        self.close_sign_in();
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        page.accounts.signing_out = Some(provider.to_owned());
        self.api.call(
            "sign_out",
            json!({"provider": provider}),
            |s, result: Result<Vec<Account>, String>, _| match result {
                Ok(list) => s.show_accounts(Ok(list)),
                Err(e) => {
                    s.fail("Couldn't sign out", e);
                    s.load_accounts();
                }
            },
        );
    }

    pub fn toggle_resource(&mut self, kind: ResourceKind, path: &str) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let Some(Ok(list)) = page.resources.as_mut() else {
            return;
        };
        let Some(item) = list.iter_mut().find(|r| r.kind == kind && r.path == path) else {
            return;
        };
        if page.resource_saving {
            return;
        }
        item.enabled = !item.enabled;
        let args = json!({"kind": kind, "path": path, "enabled": item.enabled});
        page.resource_saving = true;
        self.resources_call("set_resource", args);
    }

    fn resources_call(&mut self, tool: &'static str, args: Value) {
        self.api.call(
            tool,
            args,
            move |s, result: Result<Vec<Resource>, String>, _| {
                if let (Err(e), "set_resource") = (&result, tool) {
                    s.fail("Couldn't change pi's settings", e.clone());
                    return s.resources_call("list_resources", json!({}));
                }
                if let Some(page) = s.settings_page_mut() {
                    page.resources = Some(result);
                    page.resource_saving = false;
                }
            },
        );
    }

    pub fn close_settings(&mut self) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        let pairing = page.phone.pairing().is_some();
        let signing_in = page.accounts.sign_in.provider.is_some();
        let previous = page.previous.take();
        if signing_in {
            self.close_sign_in();
        }
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
                // The update notice may have started an install meanwhile.
                let Some(page) = s
                    .settings_page_mut()
                    .filter(|p| !matches!(p.update, Update::Installing(_)))
                else {
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

    /// Runs when a window shows, at most once a day across both windows and restarts.
    pub fn check_release(&mut self) {
        let now = Utc::now();
        let last = self.prefs.borrow().release_checked;
        if last.is_some_and(|at| now - at < RELEASE_CHECK_EVERY) {
            return;
        }
        self.prefs.borrow_mut().release_checked = Some(now);
        self.save_prefs();
        self.api.run(
            |_| async { tokio::task::spawn_blocking(update::available).await },
            move |s, result, _| match result.map_err(anyhow::Error::from).and_then(|r| r) {
                Ok(Some(version)) => {
                    if s.prefs.borrow().dismissed_release.as_ref() != Some(&version) {
                        s.release_notice = Some(version.clone());
                        s.api.peer().update(|s, _| s.release_notice = Some(version));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("checking for updates: {e:#}");
                    s.prefs.borrow_mut().release_checked = last;
                    s.save_prefs();
                }
            },
        );
    }

    pub fn dismiss_release(&mut self) {
        let Some(version) = self.release_notice.take() else {
            return;
        };
        self.prefs.borrow_mut().dismissed_release = Some(version);
        self.save_prefs();
        self.api.peer().update(|s, _| s.release_notice = None);
    }

    pub fn install_release(&mut self, tx: &mut Tx) {
        let Some(version) = self.release_notice.take() else {
            return;
        };
        if self.window == Window::Quick {
            self.api.peer().update(|s, tx| {
                s.release_notice = Some(version);
                s.install_release(tx);
            });
            return self.hide();
        }
        self.api.peer().update(|s, _| s.release_notice = None);
        self.open_settings();
        if let Some(page) = self.settings_page_mut()
            && !matches!(page.update, Update::Installing(_))
        {
            page.update = Update::Available(version);
            self.install_update(tx);
        }
    }

    #[cfg(target_os = "linux")]
    pub fn install_update(&mut self, tx: &mut Tx) {
        if let Some(page) = self.settings_page_mut()
            && let Update::Available(version) = &page.update
            && let Err(e) = tx.open_url(&update::release_page(version))
        {
            page.update = Update::Failed(format!("Couldn't open the release page: {e}"));
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    pub fn install_update(&mut self, _tx: &mut Tx) {
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

    pub fn toggle_chat_model(&mut self, id: &str) {
        let all: Vec<String> = self
            .catalog
            .ready()
            .unwrap_or_default()
            .iter()
            .map(|m| m.id.clone())
            .collect();
        self.change_settings(|v| {
            let mut picked = if v.chat_models.is_empty() {
                all.clone()
            } else {
                v.chat_models.clone()
            };
            match picked.iter().position(|m| m == id) {
                Some(at) => {
                    picked.remove(at);
                }
                None => picked.push(id.to_owned()),
            }
            // Picking every model is the same as picking none, so new models show up too.
            if all.iter().all(|m| picked.contains(m)) {
                picked.clear();
            }
            v.chat_models = picked;
        });
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

    fn load_tools(&mut self) {
        self.api.call(
            "agent_tool_status",
            json!({}),
            |s, result: Result<ToolStatuses, String>, _| {
                if let Some(page) = s.settings_page_mut() {
                    match result {
                        Ok(status) => page.tools.status = Some(status),
                        Err(e) => page.tools.failed = Some(e),
                    }
                }
            },
        );
    }

    pub fn toggle_gui_tool(&mut self, tool: GuiTool) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        if page.settings.as_ref().is_some_and(|v| v.gui_tool(tool)) {
            return self.change_settings(|v| v.set_gui_tool(tool, false));
        }
        page.tools.outcome = None;
        match page.tools.status.map(|status| status.get(tool)) {
            Some(ToolStatus::Ready) => self.change_settings(|v| v.set_gui_tool(tool, true)),
            Some(ToolStatus::Missing) => page.tools.confirm = Some(tool),
            Some(ToolStatus::NeedsPermissions | ToolStatus::Unpaired) => self.set_up_gui_tool(tool),
            None => {}
        }
    }

    pub fn cancel_gui_setup(&mut self) {
        if let Some(page) = self.settings_page_mut() {
            page.tools.confirm = None;
        }
    }

    /// Install the tool if needed, ask for its permissions or pair it, then turn it on.
    pub fn set_up_gui_tool(&mut self, tool: GuiTool) {
        let Some(page) = self.settings_page_mut() else {
            return;
        };
        page.tools.confirm = None;
        page.tools.running = Some(tool);
        self.api.call(
            "set_up_agent_tool",
            json!({"tool": tool}),
            move |s, result: Result<Setup, String>, _| {
                let Some(page) = s.settings_page_mut() else {
                    return;
                };
                page.tools.running = None;
                let outcome = match result {
                    Ok(Setup::Done(status)) => {
                        if let Some(all) = page.tools.status.as_mut() {
                            all.set(tool, status);
                        }
                        let error = match status {
                            ToolStatus::Ready => None,
                            ToolStatus::NeedsPermissions => {
                                Some("Accessibility and Screen Recording are still off.")
                            }
                            ToolStatus::Unpaired => Some("No browser has the browse extension."),
                            ToolStatus::Missing => Some("It still isn't installed."),
                        };
                        match error {
                            None => Outcome::Done,
                            Some(error) => Outcome::Failed {
                                error: error.to_owned(),
                                log: None,
                            },
                        }
                    }
                    Ok(Setup::Failed { error, log }) => Outcome::Failed { error, log },
                    Err(error) => Outcome::Failed { error, log: None },
                };
                let done = matches!(outcome, Outcome::Done);
                page.tools.outcome = Some((tool, outcome));
                if done {
                    s.change_settings(|v| v.set_gui_tool(tool, true));
                } else {
                    // A failed setup may still have installed the tool.
                    s.load_tools();
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

    pub fn confirm_unlink(&mut self, phone: Option<String>) {
        if let Some(page) = self.settings_page_mut() {
            page.phone.confirm_unlink = phone;
        }
    }

    pub fn unlink_phone(&mut self, phone: &str) {
        self.confirm_unlink(None);
        self.phone_call(
            "unlink_phone",
            json!({"phone": phone}),
            "Couldn't unlink the phone",
        );
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
