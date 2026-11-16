use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use rune::{Image, TextEdit};
use rune_kit::ComboboxState;
use rune_kit::diff::DiffState;

use super::attach::Attached;
use super::draft::Draft;
use crate::client::Subscription;
use crate::data::{
    Chat, ComposerInfo, Dialog, DialogKind, Message, Part, Queued, Role, Run, Skill, ToolState,
    TurnChanges,
};
use crate::dictation::Listening;
const INFO_EVERY: Duration = Duration::from_secs(8);
const COPIED_FOR: Duration = Duration::from_secs(2);
/// How much earlier than its send a message's own timestamp may be.
const CLOCK_SLACK_MS: i64 = 2000;

pub struct ChatPage {
    pub nav: u64,
    pub target: Target,
    pub composer: TextEdit,
    pub info: Option<ComposerInfo>,
    pub context_details: bool,
    pub overlay: Overlay,
    pub model_picker: ComboboxState,
    pub reasoning_picker: ComboboxState,
    pub skills: SkillMenu,
    pub queue_edit: Option<QueueEdit>,
    pub attached: Vec<Attached>,
    pub far_from_bottom: bool,
    pub transcript_height: f32,
    pub voice: Voice,
    /// Edits aimed at a field that's gone (an answered question) land here.
    pub(super) scratch: TextEdit,
    pub(super) next_dictation: u64,
}

pub const LEVELS_KEPT: usize = 60;

pub enum Voice {
    Idle,
    Authorizing,
    Listening(Dictating),
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
    pub heard: String,
    pub levels: VecDeque<f32>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the page is boxed whole, and is either one for its life"
)]
pub enum Target {
    New(Draft),
    Existing(Existing),
}

pub struct Existing {
    pub id: String,
    pub chat: Option<Chat>,
    pub(super) feed: Option<Subscription>,
    pub page: Option<Vec<Message>>,
    pub before: Vec<Message>,
    pub older_remaining: usize,
    pub(super) loading_earlier: bool,
    pub turn_changes: Vec<TurnChanges>,
    pub before_changes: Vec<TurnChanges>,
    pub run: Option<Run>,
    pub queued: Vec<Queued>,
    pub error: Option<String>,
    pub pending: Vec<Pending>,
    /// The locally sent prompt held at the top.
    pub top: Option<Top>,
    /// The folds the user opened (`true`) or closed, by key; the rest show
    /// as their kind does by default.
    pub open_groups: HashMap<String, bool>,
    pub wrapped_code: HashSet<String>,
    pub tool_outputs: HashMap<String, Option<Result<crate::data::ToolOutput, String>>>,
    pub question: Option<QuestionForm>,
    pub thumbs: HashMap<(String, usize), Option<Image>>,
    pub file_links: FileLinks,
    pub(super) info_at: Option<Instant>,
    pub(super) copied: Option<(String, Instant)>,
    pub changes: Option<Changes>,
    pub changes_of: ChangeTarget,
    pub details_open: bool,
    pub details: Option<Result<crate::data::ChatDetails, String>>,
    pub(super) details_loading: bool,
    pub(super) details_at: Option<Instant>,
}

/// Rune asks for inline-code links while drawing; cache filesystem lookups within the frame.
pub type FileLinks = Rc<RefCell<HashMap<String, Option<String>>>>;

#[derive(Clone, PartialEq, Eq)]
pub enum ChangeTarget {
    Turn(String),
    WorkingTree,
}

pub enum Changes {
    Loading,
    Ready {
        diff: Box<DiffState>,
        truncated: bool,
    },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Closed,
    Menu,
}

#[derive(Default)]
pub struct SkillMenu {
    pub list: SkillList,
    pub active: usize,
    pub dismissed: Option<usize>,
    pub(super) key: String,
}

#[derive(Default)]
pub enum SkillList {
    #[default]
    Loading,
    Ready(Vec<Skill>),
    Failed(String),
}

pub struct QueueEdit {
    pub id: String,
    pub draft: TextEdit,
    pub attached: Vec<Attached>,
    pub images: usize,
    pub saving: bool,
}

pub struct Top {
    /// Matches the pending prompt's send time, then pi's timestamp once it's written.
    pub at: i64,
    /// The send time. It keys the prompt's bubble, so pi's copy takes the pending one's place.
    pub sent: i64,
    /// Whether the bubble has risen into view. It mounts hidden and rises after its first
    /// layout: Rune lands entrances out of sight before the send's scroll jump brings them in.
    pub shown: bool,
}

impl Top {
    pub(super) fn new(at: i64) -> Self {
        Self {
            at,
            sent: at,
            shown: false,
        }
    }
}

pub struct Pending {
    pub text: String,
    pub images: Vec<Option<Image>>,
    /// When it was sent, in epoch milliseconds, so an older message with the
    /// same text doesn't stand for it.
    pub at: i64,
}

pub struct QuestionForm {
    pub dialog: Dialog,
    pub field: TextEdit,
    pub sending: bool,
    /// Whether the card has risen into view (see `Top::shown`).
    pub shown: bool,
}

impl ChatPage {
    pub(super) fn new(nav: u64, target: Target, composer: TextEdit) -> Self {
        Self {
            nav,
            target,
            composer,
            info: None,
            context_details: false,
            overlay: Overlay::Closed,
            model_picker: ComboboxState::default(),
            reasoning_picker: ComboboxState::default(),
            skills: SkillMenu::default(),
            queue_edit: None,
            attached: Vec::new(),
            far_from_bottom: false,
            transcript_height: 0.0,
            voice: Voice::Idle,
            scratch: TextEdit::default(),
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

    pub fn can_send(&self) -> bool {
        !self
            .existing()
            .and_then(|c| c.chat.as_ref())
            .is_some_and(|c| c.open_elsewhere)
    }

    pub fn cancel_queue_edit(&mut self) {
        if let Some(edit) = self.queue_edit.take() {
            self.composer = edit.draft;
            self.attached = edit.attached;
        }
    }
}

impl Target {
    pub(super) fn key(&self) -> &str {
        match self {
            Self::New(_) => "new",
            Self::Existing(chat) => &chat.id,
        }
    }
}

impl Existing {
    pub fn copied_key(&self) -> Option<&str> {
        self.copied
            .as_ref()
            .filter(|(_, at)| at.elapsed() < COPIED_FOR)
            .map(|(key, _)| key.as_str())
    }

    pub fn just_copied(&self, id: &str) -> bool {
        self.copied_key() == Some(id)
    }

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

    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.before.iter().chain(self.page.iter().flatten())
    }

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
            queued: Vec::new(),
            error: None,
            pending: Vec::new(),
            top: None,
            open_groups: HashMap::new(),
            wrapped_code: HashSet::new(),
            tool_outputs: HashMap::new(),
            question: None,
            thumbs: HashMap::new(),
            file_links: FileLinks::default(),
            info_at: None,
            copied: None,
            changes: None,
            changes_of: ChangeTarget::WorkingTree,
            details_open: false,
            details: None,
            details_loading: false,
            details_at: None,
        }
    }

    pub(super) fn info_due(&self) -> bool {
        self.info_at.is_none_or(|t| t.elapsed() >= INFO_EVERY)
    }

    pub fn confirm_pending(&mut self, page: &[Message], run: Option<&Run>) {
        let queued: Vec<&str> = run
            .iter()
            .flat_map(|r| r.steering.iter().chain(&r.follow_up))
            .map(|t| t.trim())
            .collect();
        if let Some(top) = &mut self.top
            && let Some(pending) = self.pending.iter().find(|p| p.at == top.at)
        {
            if let Some(message) = page
                .iter()
                .find(|m| pending.written(std::slice::from_ref(m)))
            {
                top.at = message.created_at;
            } else if pending.images.is_empty() && queued.contains(&pending.text.as_str()) {
                self.top = None;
            }
        }
        self.pending.retain(|p| {
            let arrived = p.written(page);
            !arrived && !(p.images.is_empty() && queued.contains(&p.text.as_str()))
        });
    }
}

impl Pending {
    pub(super) fn written(&self, page: &[Message]) -> bool {
        page.iter().any(|m| {
            m.role == Role::User
                && m.created_at >= self.at - CLOCK_SLACK_MS
                && m.text().trim() == self.text
                && m.images().count() == self.images.len()
        })
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
            shown: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::MessageStatus;

    #[test]
    fn previews_clear_only_after_matching_delivery() {
        let typed = "[Image 1](</tmp/image.png>) - review this.\nPlease $commit the fix";
        let mut chat = Existing::new("chat".into(), None);
        chat.pending.push(Pending {
            text: typed.into(),
            images: vec![None],
            at: 10_000,
        });
        let mut message = Message {
            id: "user".into(),
            role: Role::User,
            created_at: 10_100,
            ended_at: 10_100,
            status: MessageStatus::Complete,
            parts: vec![Part::Text { text: typed.into() }, Part::Image { index: 0 }],
        };
        message.created_at = 1;
        chat.confirm_pending(std::slice::from_ref(&message), None);
        assert_eq!(chat.pending.len(), 1, "an old message is not this send");

        message.created_at = 10_100;
        message.parts.pop();
        chat.confirm_pending(std::slice::from_ref(&message), None);
        assert_eq!(chat.pending.len(), 1, "the attachment has not arrived");

        message.parts.push(Part::Image { index: 0 });
        chat.confirm_pending(std::slice::from_ref(&message), None);
        assert!(
            chat.pending.is_empty(),
            "the delivered skill still has a preview"
        );
        chat.confirm_pending(&[message], None);
        assert!(
            chat.pending.is_empty(),
            "later frames keep the preview cleared"
        );

        chat.pending.push(Pending {
            text: "please $commit".into(),
            images: Vec::new(),
            at: 10_000,
        });
        chat.confirm_pending(&[], None);
        assert_eq!(chat.pending.len(), 1, "keep the preview until delivery");
        chat.confirm_pending(
            &[],
            Some(&Run {
                steering: vec!["please $commit".into()],
                ..Run::default()
            }),
        );
        assert!(chat.pending.is_empty(), "pi already shows the queued send");
    }
}
