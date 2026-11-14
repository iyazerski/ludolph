//! A chat's page: an existing chat, or a new one until its first message
//! creates it, under the composer they share.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use rune::{Image, TextEdit};
use rune_kit::ComboboxState;
use rune_kit::diff::DiffState;

use super::attach::Attached;
use super::draft::Draft;
use crate::client::Subscription;
use crate::data::{
    Chat, ComposerInfo, Dialog, DialogKind, Message, Part, Role, Run, Skill, ToolState, TurnChanges,
};
use crate::dictation::Listening;
/// The composer's branch and context change slowly; re-read them at most this often.
const INFO_EVERY: Duration = Duration::from_secs(8);
/// How long a reply's copy button shows a check after a copy.
const COPIED_FOR: Duration = Duration::from_secs(2);

pub struct ChatPage {
    pub nav: u64,
    pub target: Target,
    pub composer: TextEdit,
    pub info: Option<ComposerInfo>,
    pub overlay: Overlay,
    /// Inline model and reasoning menus, shared by both windows.
    pub model_picker: ComboboxState,
    pub reasoning_picker: ComboboxState,
    pub skills: SkillMenu,
    /// Images in the composer, read as they're attached.
    pub attached: Vec<Attached>,
    pub far_from_bottom: bool,
    pub voice: Voice,
    /// Edits aimed at a field that's gone (an answered question) land here.
    pub(super) scratch: TextEdit,
    /// The next attached image's id; ids are never reused, so a late upload
    /// can't land on a newer image.
    next_attached: u64,
    /// The next dictation's id, so a session that ended can't write.
    pub(super) next_dictation: u64,
}

/// How many recent loudness readings the waveform shows.
pub const LEVELS_KEPT: usize = 60;

/// Where dictation into the composer is.
pub enum Voice {
    Idle,
    /// Asking for the microphone and speech recognition.
    Authorizing,
    Listening(Dictating),
    /// What dictation heard is being cleaned up.
    Polishing,
}

impl Voice {
    pub fn listening(&self) -> Option<&Dictating> {
        match self {
            Self::Listening(d) => Some(d),
            Self::Idle | Self::Authorizing | Self::Polishing => None,
        }
    }
}

pub struct Dictating {
    pub id: u64,
    pub session: Listening,
    pub started: Instant,
    /// What's heard so far, shown only once cleaned up.
    pub heard: String,
    /// Recent loudness, 0 to 1, oldest first.
    pub levels: VecDeque<f32>,
}

/// What the page shows above the composer.
#[expect(
    clippy::large_enum_variant,
    reason = "the page is boxed whole, and is either one for its life"
)]
pub enum Target {
    /// A chat that the first message creates.
    New(Draft),
    Existing(Existing),
}

pub struct Existing {
    pub id: String,
    pub chat: Option<Chat>,
    /// The chat's feed, while the window shows it.
    pub(super) feed: Option<Subscription>,
    /// The feed's page, from its first frame on.
    pub page: Option<Vec<Message>>,
    /// Earlier messages loaded before the page, until the feed resets.
    pub before: Vec<Message>,
    /// How many messages come before the first one shown.
    pub older_remaining: usize,
    pub(super) loading_earlier: bool,
    /// The shown runs that changed files: the page's, then the earlier ones'.
    pub turn_changes: Vec<TurnChanges>,
    pub before_changes: Vec<TurnChanges>,
    pub run: Option<Run>,
    pub error: Option<String>,
    /// Sent messages not in the transcript yet.
    pub pending: Vec<Pending>,
    /// The folds the user opened (`true`) or closed, by key; the rest show
    /// as their kind does by default.
    pub open_groups: HashMap<String, bool>,
    pub question: Option<QuestionForm>,
    /// The shown messages' images, by message and index, decoded once.
    pub thumbs: HashMap<(String, usize), Option<Image>>,
    pub file_links: FileLinks,
    /// When the composer's info was last read, so it's read again only
    /// when a run may have changed it.
    pub(super) info_at: Option<Instant>,
    /// The message whose answer was just copied, and when.
    pub(super) copied: Option<(String, Instant)>,
    /// The latest turn's diff, shown in place of the transcript while open.
    pub changes: Option<Changes>,
    /// The turn the changes page shows: the user message that started it.
    pub changes_of: String,
}

/// The file each inline code names, by its text, looked up once per
/// frame. The view fills it, as Rune's Markdown asks for links while it draws.
pub type FileLinks = Rc<RefCell<HashMap<String, Option<String>>>>;

pub enum Changes {
    Loading,
    Ready {
        diff: Box<DiffState>,
        /// The engine capped the patch, so files or lines are missing.
        truncated: bool,
    },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Closed,
    Menu,
    Sheet(Sheet),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sheet {
    Project,
}

#[derive(Default)]
pub struct SkillMenu {
    pub list: SkillList,
    pub active: usize,
    /// Escape closed the menu for the `$` word at this byte.
    pub dismissed: Option<usize>,
    /// The scope the list was loaded for.
    pub(super) key: String,
}

/// The skills of the page's agent, for the `$` menu.
#[derive(Default)]
pub enum SkillList {
    #[default]
    Loading,
    Ready(Vec<Skill>),
    Failed(String),
}

impl SkillList {
    pub fn ready(&self) -> &[Skill] {
        match self {
            Self::Ready(skills) => skills,
            Self::Loading | Self::Failed(_) => &[],
        }
    }
}

/// A message sent but not in the transcript yet.
pub struct Pending {
    pub text: String,
    pub images: Vec<Option<Image>>,
    /// When it was sent, in epoch milliseconds, so an older message with the
    /// same text doesn't stand for it.
    pub(super) at: i64,
}

/// The dialog pi waits on, with what's typed into it.
pub struct QuestionForm {
    pub dialog: Dialog,
    pub field: TextEdit,
    pub sending: bool,
}

impl ChatPage {
    pub(super) fn new(nav: u64, target: Target, composer: TextEdit) -> Self {
        Self {
            nav,
            target,
            composer,
            info: None,
            overlay: Overlay::Closed,
            model_picker: ComboboxState::default(),
            reasoning_picker: ComboboxState::default(),
            skills: SkillMenu::default(),
            attached: Vec::new(),
            far_from_bottom: false,
            voice: Voice::Idle,
            scratch: TextEdit::default(),
            next_attached: 0,
            next_dictation: 0,
        }
    }

    pub fn existing(&self) -> Option<&Existing> {
        match &self.target {
            Target::Existing(chat) => Some(chat),
            Target::New(_) => None,
        }
    }

    pub fn existing_mut(&mut self) -> Option<&mut Existing> {
        match &mut self.target {
            Target::Existing(chat) => Some(chat),
            Target::New(_) => None,
        }
    }

    pub fn draft(&self) -> Option<&Draft> {
        match &self.target {
            Target::New(draft) => Some(draft),
            Target::Existing(_) => None,
        }
    }

    pub fn draft_mut(&mut self) -> Option<&mut Draft> {
        match &mut self.target {
            Target::New(draft) => Some(draft),
            Target::Existing(_) => None,
        }
    }

    pub fn is_working(&self) -> bool {
        self.existing()
            .and_then(|c| c.chat.as_ref())
            .is_some_and(Chat::is_working)
    }

    /// Whether this Mac can send to the chat: not while a terminal pi has it open.
    pub fn can_send(&self) -> bool {
        !self
            .existing()
            .and_then(|c| c.chat.as_ref())
            .is_some_and(|c| c.open_elsewhere)
    }

    pub(super) fn attached_id(&mut self) -> u64 {
        self.next_attached += 1;
        self.next_attached
    }
}

impl Target {
    /// Where the page's unsent text is kept: the chat's id, or `new`.
    pub(super) fn key(&self) -> &str {
        match self {
            Self::New(_) => "new",
            Self::Existing(chat) => &chat.id,
        }
    }
}

impl Existing {
    /// Whether message `id`'s answer was copied a moment ago.
    pub fn just_copied(&self, id: &str) -> bool {
        self.copied
            .as_ref()
            .is_some_and(|(m, at)| m == id && at.elapsed() < COPIED_FOR)
    }

    /// Forget a copy shown long enough; true when one was.
    pub(super) fn expire_copied(&mut self) -> bool {
        let expired = self
            .copied
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= COPIED_FOR);
        if expired {
            self.copied = None;
        }
        expired
    }

    /// The shown messages, oldest first.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.before.iter().chain(self.page.iter().flatten())
    }

    /// Whether the agent works but shows nothing yet (no prose and no
    /// running tool), so the transcript counts the seconds instead.
    pub fn shows_clock(&self) -> bool {
        let working = self.chat.as_ref().is_some_and(Chat::is_working);
        working
            && self.question.is_none()
            && self.page.as_ref().is_some_and(|page| {
                page.last().is_none_or(|m| {
                    m.role == Role::User
                        || !m.parts.iter().any(|p| match p {
                            Part::Text { .. } => true,
                            Part::Tool { state, .. } => *state == ToolState::Running,
                            Part::Thinking { .. }
                            | Part::Image { .. }
                            | Part::Error { .. }
                            | Part::Note { .. } => false,
                        })
                })
            })
    }

    pub(super) fn new(id: String, chat: Option<Chat>) -> Self {
        Self {
            id,
            chat,
            feed: None,
            page: None,
            before: Vec::new(),
            older_remaining: 0,
            loading_earlier: false,
            turn_changes: Vec::new(),
            before_changes: Vec::new(),
            run: None,
            error: None,
            pending: Vec::new(),
            open_groups: HashMap::new(),
            question: None,
            thumbs: HashMap::new(),
            file_links: FileLinks::default(),
            info_at: None,
            copied: None,
            changes: None,
            changes_of: String::new(),
        }
    }

    /// Whether the composer's info is due again: a run may have changed the
    /// model or the context, and it's read at most every [`INFO_EVERY`].
    pub(super) fn info_due(&self) -> bool {
        self.info_at.is_none_or(|t| t.elapsed() >= INFO_EVERY)
    }
}

impl QuestionForm {
    pub(super) fn new(dialog: Dialog) -> Self {
        let typed = match dialog.dialog {
            DialogKind::Input | DialogKind::Editor => dialog.prefill.clone().unwrap_or_default(),
            DialogKind::Select | DialogKind::Confirm => String::new(),
        };
        Self {
            field: TextEdit::new(typed),
            dialog,
            sending: false,
        }
    }
}
