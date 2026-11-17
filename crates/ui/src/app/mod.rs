mod actions;
mod attach;
mod chats;
mod details;
mod draft;
mod page;
mod queue;
mod settings;
#[cfg(test)]
mod ui_tests;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rune::{ImageCache, ShortcutId, TextEdit, Tx, WindowHandle};
use rune_kit::diff::DiffState;
use rune_kit::shell::PaneState;
use serde_json::{Value, json};

use super::api::Api;
use super::client::Subscription;
use super::data::{Chat, Chats};
use super::prefs::{Frame, Prefs, ProjectPick, TextSize};

pub use self::actions::Answer;
pub use self::attach::Prep;
pub use self::chats::{ChatDialog, project_drop_key};
pub use self::draft::{Catalog, Config, Draft, Load, Pick};
pub use self::page::{
    ChangeTarget, Changes, ChatPage, Existing, FileLinks, LEVELS_KEPT, Overlay, Pending,
    QuestionForm, SkillList, Target, Top, Voice,
};
pub use self::settings::{Outcome, QR_SIZE, Section, SettingsPage, Update};

const LIST_LIMIT: usize = 200;
pub const GROUP_CHATS: usize = 5;
const TOAST_FOR: Duration = Duration::from_millis(3800);

#[derive(Default)]
struct MessageDraft {
    text: String,
    attached: Vec<attach::Attached>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Quick,
    Main,
}

pub struct State {
    pub api: Api,
    pub window: Window,
    pub engine_up: bool,
    pub prefs: Rc<RefCell<Prefs>>,
    pub hotkey: Rc<Cell<Option<ShortcutId>>>,
    pub expand_tools: Rc<Cell<bool>>,
    handle: Rc<OnceCell<WindowHandle>>,
    pub page: Page,
    pub sidebar: PaneState,
    pub list: ListState,
    pub toast: Option<Toast>,
    /// A newer release to offer; it stays until updated or closed.
    pub release_notice: Option<String>,
    pub chat_dialog: Option<ChatDialog>,
    /// Edits aimed at the rename field once its dialog is gone land here.
    scratch_title: TextEdit,
    pub catalog: Catalog,
    pub images: Option<ImageCache>,
    /// Unsent drafts (`new` for a new chat), kept while moving between chats.
    drafts: HashMap<String, MessageDraft>,
    /// Unique across pages so an image finishing preparation can find its saved draft.
    next_attached: u64,
    /// Page generation: discard replies for pages the user has left.
    nav: u64,
    /// What the diff view reads and changes once its page is gone.
    scratch_diff: DiffState,
    list_feed: Option<Subscription>,
    models_feed: Option<Subscription>,
    shown: bool,
}

pub enum Page {
    List,
    Chat(Box<ChatPage>),
    Settings(Box<SettingsPage>),
}

#[derive(Default)]
pub struct ListState {
    pub chats: Option<Vec<Chat>>,
    pub query: TextEdit,
    pub filter: ListFilter,
    pub error: Option<String>,
    pub more: HashMap<String, usize>,
    pub drag_chat: Option<String>,
    pub drop_project: Option<String>,
    loading: bool,
    stale: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListFilter {
    #[default]
    All,
    Working,
    Done,
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
        expand_tools: Rc<Cell<bool>>,
        handle: Rc<OnceCell<WindowHandle>>,
    ) -> Self {
        let page = match window {
            Window::Quick => Page::List,
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
            expand_tools,
            handle,
            page,
            sidebar: PaneState::new(320.0).range(260.0, 480.0),
            list: ListState::default(),
            toast: None,
            release_notice: None,
            chat_dialog: None,
            scratch_title: TextEdit::default(),
            catalog: Catalog::default(),
            images: None,
            drafts: HashMap::new(),
            next_attached: 0,
            nav: 0,
            scratch_diff: DiffState::default(),
            list_feed: None,
            models_feed: None,
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

    pub fn set_text_size(&mut self, size: TextSize) {
        self.prefs.borrow_mut().text_size = size;
        self.save_prefs();
    }

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
            self.check_release();
            self.follow();
            self.load_details();
            if self.settings_page().is_some() {
                self.load_phone();
            }
        } else {
            self.models_feed = None;
            self.list_feed = None;
            if let Some(chat) = self.chat_mut().and_then(ChatPage::existing_mut) {
                chat.feed = None;
            }
        }
        self.sync_tick();
    }

    fn follow(&mut self) {
        if !self.shown {
            return;
        }
        if self.models_feed.is_none() {
            self.models_feed = Some(self.api.subscribe("models", json!({}), |s, _, _| {
                s.refresh_catalog();
            }));
            self.refresh_catalog();
        }
        if self.list_feed.is_none() {
            self.list_feed = Some(self.api.subscribe("chats", json!({}), |s, _, _| {
                if s.list_shown() {
                    s.load_chats();
                } else {
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
        let details = self.engine_up && self.details_visible();
        self.api
            .tick(self.shown && (toast || chat || linking || details));
    }

    pub fn toggle_window(&mut self) {
        if self.api.app().is_visible() {
            return self.hide();
        }
        self.open();
    }

    pub fn open(&mut self) {
        self.api.app().open();
        // Rune reports visibility changes only after the first frame, so the
        // first open must count itself.
        self.visibility_changed(true);
        if let Some(page) = self.chat_mut() {
            page.composer.focused = true;
        }
    }

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

    pub fn closed(&mut self, _: &mut Tx) {
        self.keep_frame();
        self.stop_dictating();
        self.visibility_changed(false);
        self.api.app().set_dock_icon(false);
        // A hidden app can stay alive for days; drop its transcript but preserve unsent drafts.
        self.new_chat();
    }

    pub fn keep_frame(&mut self) {
        if self.window != Window::Main {
            return;
        }
        let Some(at) = self.handle.get().and_then(WindowHandle::placement) else {
            return;
        };
        if at.minimized || at.full_screen {
            return;
        }
        self.prefs.borrow_mut().main_frame = Some(Frame {
            position: at.position,
            size: at.content_size,
        });
        self.save_prefs();
    }

    pub fn open_in_peer(&mut self, id: String) {
        self.api.peer().update(move |s, _| s.present(&id));
        if self.window == Window::Quick {
            self.hide();
        }
    }

    pub fn open_main(&mut self) {
        self.api.peer().update(|s, _| s.open());
        self.hide();
    }

    pub fn present(&mut self, id: &str) {
        if !self.shows_chat(id) {
            self.open_chat(id.to_owned());
        }
        self.open();
    }

    pub fn engine_changed(&mut self, up: bool) {
        let came = up && !self.engine_up;
        self.engine_up = up;
        if !came {
            return;
        }
        self.load_expand_tools();
        if self.shown {
            self.refresh_catalog();
            self.load_chats();
        }
        if let Some(Target::New(draft)) = self.chat().map(|p| &p.target)
            && draft.load != Load::Ready
        {
            self.load_draft();
        }
    }

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
            changed |= chat.shows_clock() || chat.run.as_ref().is_some_and(|r| r.retry.is_some());
        }
        self.load_details();
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
        self.load_details();
        self.sync_tick();
    }

    pub fn shows_chat(&self, id: &str) -> bool {
        self.chat()
            .and_then(ChatPage::existing)
            .is_some_and(|c| c.id == id)
    }

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

    pub fn back(&mut self) {
        if let Some(chat) = self.chat_mut().and_then(ChatPage::existing_mut)
            && chat.changes.take().is_some()
        {
            self.load_details();
            self.sync_tick();
            return;
        }
        if let Some(chat) = self.chat_mut().and_then(ChatPage::existing_mut)
            && chat.details_open
        {
            chat.details_open = false;
            self.sync_tick();
            return;
        }
        if self.window == Window::Quick {
            self.leave_chat();
        }
    }

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

    fn show(&mut self, target: Target) {
        self.leave_page();
        self.nav += 1;
        let saved = self.drafts.remove(target.key()).unwrap_or_default();
        let mut composer = TextEdit::new(saved.text);
        composer.set_caret(composer.text.len());
        composer.focused = true;
        let mut page = ChatPage::new(self.nav, target, composer);
        page.attached = saved.attached;
        self.page = Page::Chat(Box::new(page));
    }

    fn leave_page(&mut self) {
        let Page::Chat(mut page) = std::mem::replace(&mut self.page, Page::List) else {
            return;
        };
        page.cancel_queue_edit();
        let key = page.target.key();
        if page.composer.text.trim().is_empty() && page.attached.is_empty() {
            self.drafts.remove(key);
        } else {
            let text = std::mem::take(&mut page.composer.text);
            self.drafts.insert(
                key.to_owned(),
                MessageDraft {
                    text,
                    attached: page.attached,
                },
            );
        }
    }
}
