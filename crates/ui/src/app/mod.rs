//! A window's state and what changes it: navigation, the engine's feeds
//! and the windows' hand-offs here, a chat's actions in [`actions`], a new chat's picks in
//! [`draft`], images in [`attach`], and the settings page in [`settings`].

mod actions;
mod attach;
mod draft;
mod page;
mod settings;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rune::{ImageCache, ShortcutId, TextEdit, Tx};
use rune_kit::diff::DiffState;
use rune_kit::shell::PaneState;
use serde_json::{Value, json};

use super::api::Api;
use super::client::Subscription;
use super::data::{Chat, Chats};
use super::prefs::{Prefs, ProjectPick};

pub use self::actions::Answer;
pub use self::attach::Prep;
pub use self::draft::{Catalog, Config, Draft, Load, Pick};
pub use self::page::{
    Changes, ChatPage, Existing, FileLinks, LEVELS_KEPT, Overlay, Pending, QuestionForm, Sheet,
    SkillList, Target, Voice,
};
pub use self::settings::SettingsPage;

/// The most chats the list loads, for its search.
const LIST_LIMIT: usize = 200;
/// Chats a project shows at first, and how many more "Show more" adds.
pub const GROUP_CHATS: usize = 10;
const TOAST_FOR: Duration = Duration::from_millis(3800);

/// Which of the app's two windows a state belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// The floating window on the global shortcut, which pages between the
    /// list and a chat.
    Quick,
    /// The regular window, with the list in a sidebar beside the chat.
    Main,
}

pub struct State {
    pub api: Api,
    pub window: Window,
    /// Whether the app is connected to the engine.
    pub engine_up: bool,
    /// Shared by both windows, which run on the same thread.
    pub prefs: Rc<RefCell<Prefs>>,
    /// The quick window's global shortcut while it's registered, shared too:
    /// the main window's settings change it.
    pub hotkey: Rc<Cell<Option<ShortcutId>>>,
    pub page: Page,
    /// The main window's sidebar: its width, and whether it's open.
    pub sidebar: PaneState,
    pub list: ListState,
    pub toast: Option<Toast>,
    /// pi's models and their thinking levels, loaded once.
    pub catalog: Catalog,
    /// Decodes images off the UI thread; set once the app starts.
    pub images: Option<ImageCache>,
    /// Unsent text per chat (`new` for a new chat), kept while moving between chats.
    drafts: HashMap<String, String>,
    /// Counts page changes, so a late answer for a page that's gone is dropped.
    nav: u64,
    /// What the diff view reads and changes once its page is gone.
    scratch_diff: DiffState,
    /// The chat list's notices, while the window shows.
    list_feed: Option<Subscription>,
    /// Whether the window is on screen.
    shown: bool,
}

pub enum Page {
    List,
    Chat(Box<ChatPage>),
    /// Only the main window shows the settings.
    Settings(Box<SettingsPage>),
}

#[derive(Default)]
pub struct ListState {
    pub chats: Option<Vec<Chat>>,
    pub query: TextEdit,
    pub filter: ListFilter,
    pub error: Option<String>,
    /// How many chats each expanded project shows, by its key.
    pub more: HashMap<String, usize>,
    loading: bool,
    /// The list changed while it loaded, so it loads again after.
    stale: bool,
}

/// Which chats the list shows, by status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListFilter {
    #[default]
    All,
    Working,
    /// Finished since they were last opened: the ones marked Done.
    Done,
    /// Active in the last `RECENT`.
    Recent,
}

impl ListFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Working, Self::Done, Self::Recent];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Working => "Working",
            Self::Done => "Done",
            Self::Recent => "Recent",
        }
    }
}

pub struct Toast {
    pub title: String,
    pub body: String,
    pub kind: ToastKind,
    time: ToastTime,
}

/// A toast's time left on screen, held while the window is hidden.
#[derive(Clone, Copy)]
enum ToastTime {
    Until(Instant),
    Held(Duration),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Note,
    Failure,
}

impl State {
    pub fn new(
        api: Api,
        window: Window,
        prefs: Rc<RefCell<Prefs>>,
        hotkey: Rc<Cell<Option<ShortcutId>>>,
    ) -> Self {
        let page = match window {
            Window::Quick => Page::List,
            // The main window opens on a new chat, as there's always a page beside the list.
            Window::Main => Page::Chat(Box::new(ChatPage::new(
                0,
                Target::New(Draft::new(prefs.borrow().new_chat.clone())),
                TextEdit::default(),
            ))),
        };
        Self {
            api,
            window,
            engine_up: false,
            prefs,
            hotkey,
            page,
            sidebar: PaneState::new(320.0).range(260.0, 480.0),
            list: ListState::default(),
            toast: None,
            catalog: Catalog::default(),
            images: None,
            drafts: HashMap::new(),
            nav: 0,
            scratch_diff: DiffState::default(),
            list_feed: None,
            shown: false,
        }
    }

    pub fn chat(&self) -> Option<&ChatPage> {
        match &self.page {
            Page::Chat(page) => Some(page),
            Page::List | Page::Settings(_) => None,
        }
    }

    pub fn chat_mut(&mut self) -> Option<&mut ChatPage> {
        match &mut self.page {
            Page::Chat(page) => Some(page),
            Page::List | Page::Settings(_) => None,
        }
    }

    pub fn composer_field(&mut self) -> &mut TextEdit {
        match &mut self.page {
            Page::Chat(page) => &mut page.composer,
            Page::List | Page::Settings(_) => &mut self.list.query,
        }
    }

    pub fn answer_field(&mut self) -> &mut TextEdit {
        match &mut self.page {
            Page::Chat(page) => match &mut page.target {
                Target::Existing(Existing {
                    question: Some(form),
                    ..
                }) => &mut form.field,
                _ => &mut page.scratch,
            },
            Page::List | Page::Settings(_) => &mut self.list.query,
        }
    }

    pub fn diff(&self) -> &DiffState {
        match self
            .chat()
            .and_then(ChatPage::existing)
            .and_then(|c| c.changes.as_ref())
        {
            Some(Changes::Ready { diff, .. }) => diff,
            _ => &self.scratch_diff,
        }
    }

    pub fn diff_mut(&mut self) -> &mut DiffState {
        let changes = match &mut self.page {
            Page::Chat(page) => page.existing_mut().and_then(|c| c.changes.as_mut()),
            Page::List | Page::Settings(_) => None,
        };
        match changes {
            Some(Changes::Ready { diff, .. }) => diff,
            _ => &mut self.scratch_diff,
        }
    }

    pub fn page_at(&mut self, nav: u64) -> Option<&mut ChatPage> {
        self.chat_mut().filter(|p| p.nav == nav)
    }

    fn existing_at(&mut self, nav: u64) -> Option<&mut Existing> {
        self.page_at(nav)?.existing_mut()
    }

    pub fn save_prefs(&self) {
        self.api.save_prefs(&self.prefs.borrow());
    }

    /// Whether the window shows the chat list now: the main window's
    /// sidebar always does, the quick window only in place of a chat.
    pub fn list_shown(&self) -> bool {
        self.window == Window::Main || self.chat().is_none()
    }

    pub fn notify(&mut self, title: &str) {
        self.show_toast(ToastKind::Note, title, String::new());
    }

    pub fn fail(&mut self, title: &str, why: impl Into<String>) {
        self.show_toast(ToastKind::Failure, title, why.into());
    }

    fn show_toast(&mut self, kind: ToastKind, title: &str, body: String) {
        self.toast = Some(Toast {
            title: title.to_owned(),
            body,
            kind,
            time: if self.shown {
                ToastTime::Until(Instant::now() + TOAST_FOR)
            } else {
                ToastTime::Held(TOAST_FOR)
            },
        });
        self.sync_tick();
    }

    /// Follow the engine while the window shows, and let it rest while it's
    /// hidden: a hidden window has nothing to redraw.
    pub fn visibility_changed(&mut self, shown: bool) {
        self.shown = shown;
        if let Some(toast) = &mut self.toast {
            let now = Instant::now();
            toast.time = match toast.time {
                ToastTime::Until(until) if !shown => {
                    ToastTime::Held(until.saturating_duration_since(now))
                }
                ToastTime::Held(left) if shown => ToastTime::Until(now + left),
                time => time,
            };
        }
        if shown {
            self.follow();
        } else {
            self.list_feed = None;
            if let Some(chat) = self.chat_mut().and_then(ChatPage::existing_mut) {
                chat.feed = None;
            }
        }
        self.sync_tick();
    }

    /// Subscribe to what the shown page needs, if it isn't yet.
    fn follow(&mut self) {
        if !self.shown {
            return;
        }
        if self.list_feed.is_none() {
            self.list_feed = Some(self.api.subscribe("chats", json!({}), |s, _, _| {
                if s.list_shown() {
                    s.load_chats();
                } else {
                    // The list loads again once it shows.
                    s.list.stale = true;
                }
            }));
            self.load_chats();
        }
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut().filter(|c| c.feed.is_none()) else {
            return;
        };
        let args = json!({"chat": chat.id, "thinking": true});
        let feed = self
            .api
            .subscribe("chat", args, move |s, frame: Value, tx| {
                s.frame(nav, frame, tx);
            });
        if let Some(chat) = self.existing_at(nav) {
            chat.feed = Some(feed);
        }
    }

    /// Tick each second while something on screen counts: a toast's time,
    /// the working clock, a copy's check, a retry's countdown, a phone link
    /// connecting.
    pub(super) fn sync_tick(&mut self) {
        let toast = self
            .toast
            .as_ref()
            .is_some_and(|t| matches!(t.time, ToastTime::Until(_)));
        let chat = self.chat().and_then(ChatPage::existing).is_some_and(|c| {
            c.shows_clock()
                || c.copied.is_some()
                || c.run.as_ref().is_some_and(|r| r.retry.is_some())
        });
        let linking = self.settings_page().is_some_and(SettingsPage::connecting);
        self.api.tick(self.shown && (toast || chat || linking));
    }

    /// Show or hide the window; it opens where it was left.
    pub fn toggle_window(&mut self) {
        if self.api.app().is_visible() {
            return self.hide();
        }
        self.open();
    }

    /// Bring the window forward.
    pub fn open(&mut self) {
        self.api.app().open();
        // Rune reports visibility changes only after the first frame, so the
        // first open must count itself.
        self.visibility_changed(true);
        if let Some(page) = self.chat_mut() {
            page.composer.focused = true;
        }
    }

    /// Put the window away: the quick window hides, and the main window
    /// closes as its close button would, keeping its state.
    pub fn hide(&mut self) {
        match self.window {
            Window::Quick => {
                self.stop_dictating();
                self.api.app().hide();
                self.visibility_changed(false);
            }
            Window::Main => self.api.app().close(),
        }
    }

    /// The main window closed: it rests, and the app leaves the Dock, as
    /// the quick window and the shortcut stay.
    pub fn closed(&mut self, _: &mut Tx) {
        self.stop_dictating();
        self.visibility_changed(false);
        self.api.app().set_dock_icon(false);
        // Its transcript is most of its memory, and the app may stay hidden for
        // days. It opens again on a new chat, keeping any unsent text.
        self.new_chat();
    }

    /// Show chat `id` in the other window. Expanding the quick window into
    /// the main one puts it away; the main window stays behind the quick one.
    pub fn open_in_peer(&mut self, id: String) {
        self.api.peer().update(move |s, _| s.present(&id));
        if self.window == Window::Quick {
            self.hide();
        }
    }

    /// Bring the main window forward from the quick window's list, which hides.
    pub fn open_main(&mut self) {
        self.api.peer().update(|s, _| s.open());
        self.hide();
    }

    /// Bring the window forward on chat `id`, keeping its page if it shows it.
    pub fn present(&mut self, id: &str) {
        if !self.shows_chat(id) {
            self.open_chat(id.to_owned());
        }
        self.open();
    }

    /// The engine came or went. Feeds follow again on their own; what was
    /// loaded once loads again.
    pub fn engine_changed(&mut self, up: bool) {
        let came = up && !self.engine_up;
        self.engine_up = up;
        if !came {
            return;
        }
        if self.shown {
            self.load_chats();
        }
        if let Some(Target::New(draft)) = self.chat().map(|p| &p.target)
            && draft.load == Load::Failed
        {
            self.load_draft();
        }
    }

    /// Start what's due and expire timed state; true when the view must redraw.
    pub fn tick(&mut self, _: &mut Tx) -> bool {
        let mut changed = false;
        if self
            .toast
            .as_ref()
            .is_some_and(|t| matches!(t.time, ToastTime::Until(until) if Instant::now() >= until))
        {
            self.toast = None;
            changed = true;
        }
        if self.settings_page().is_some_and(SettingsPage::connecting) {
            self.load_phone();
        }
        if let Some(chat) = self.chat_mut().and_then(|p| p.existing_mut()) {
            changed |= chat.expire_copied();
            // The working clock and a retry's countdown count seconds.
            changed |= chat.shows_clock() || chat.run.as_ref().is_some_and(|r| r.retry.is_some());
        }
        self.sync_tick();
        changed
    }

    fn load_chats(&mut self) {
        if self.list.loading {
            self.list.stale = true;
            return;
        }
        self.list.loading = true;
        self.list.stale = false;
        self.api.call(
            "list_chats",
            json!({ "limit": LIST_LIMIT }),
            |s, result: Result<Chats, String>, _| {
                s.list.loading = false;
                if s.list.stale && s.list_shown() {
                    s.load_chats();
                }
                match result {
                    Ok(mut got) => {
                        got.chats.sort_by_key(|c| std::cmp::Reverse(c.active_at()));
                        let primed = s.prefs.borrow_mut().prime(&got.chats);
                        if primed {
                            s.save_prefs();
                        }
                        s.list.chats = Some(got.chats);
                        s.list.error = None;
                    }
                    Err(e) => s.list.error = Some(e),
                }
            },
        );
    }

    pub fn open_chat(&mut self, id: String) {
        let chat = self
            .list
            .chats
            .iter()
            .flatten()
            .find(|c| c.id == id)
            .cloned();
        self.show(Target::Existing(Existing::new(id, chat)));
        self.follow();
        self.load_info();
    }

    /// Whether the window shows chat `id`'s page.
    pub fn shows_chat(&self, id: &str) -> bool {
        self.chat()
            .and_then(ChatPage::existing)
            .is_some_and(|c| c.id == id)
    }

    /// Start a new chat in project `id`, with the last new chat's model.
    pub fn new_chat_in(&mut self, id: String) {
        let mut saved = self.prefs.borrow().new_chat.clone();
        saved.project = ProjectPick::Project(id);
        self.show(Target::New(Draft::new(saved)));
        self.load_draft();
    }

    pub fn new_chat(&mut self) {
        let saved = self.prefs.borrow().new_chat.clone();
        self.show(Target::New(Draft::new(saved)));
        self.load_draft();
    }

    /// Close the changes page for its chat, else, in the quick window, the
    /// chat for the list. The main window's list is always beside the chat.
    pub fn back(&mut self) {
        if let Some(chat) = self.chat_mut().and_then(ChatPage::existing_mut)
            && chat.changes.take().is_some()
        {
            return;
        }
        if self.window == Window::Quick {
            self.leave_chat();
        }
    }

    /// Leave the chat: for the list in the quick window, for a new chat in
    /// the main one.
    pub fn leave_chat(&mut self) {
        match self.window {
            Window::Quick => {
                self.leave_page();
                if self.list.stale {
                    self.load_chats();
                }
            }
            Window::Main => self.new_chat(),
        }
    }

    /// Show `target`'s page, with the text left unsent there before.
    fn show(&mut self, target: Target) {
        self.leave_page();
        self.nav += 1;
        let mut composer = TextEdit::new(self.drafts.remove(target.key()).unwrap_or_default());
        composer.set_caret(composer.text.len());
        composer.focused = true;
        self.page = Page::Chat(Box::new(ChatPage::new(self.nav, target, composer)));
    }

    fn leave_page(&mut self) {
        let Page::Chat(mut page) = std::mem::replace(&mut self.page, Page::List) else {
            return;
        };
        let key = page.target.key();
        if page.composer.text.trim().is_empty() {
            self.drafts.remove(key);
        } else {
            let text = std::mem::take(&mut page.composer.text);
            self.drafts.insert(key.to_owned(), text);
        }
    }
}
