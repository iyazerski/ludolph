//! The engine's chats: which have a pi process, what they're doing, and the
//! commands that start, steer and stop their runs.
//!
//! A chat is a pi session, named by its session id. Its pi starts when the
//! chat first needs one and stops after a few idle minutes (`docs/design.md`,
//! "pi processes").

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::{OwnedMutexGuard, broadcast, mpsc, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::index::{self, Index};
use crate::live::{self, Live};
use crate::login;
use crate::models::{Catalog, Model};
use crate::pi::{Event, Options, Pi, Session};
pub use crate::relay::SecretStore;
use crate::relay::{self, DEFAULT_RELAY, Link, LinkStatus, Pairing, Phone, Phones};
use crate::runtime::Runtime;
use crate::seal;
use crate::session::{Outcome, Spent, Summary};
use crate::settings::Settings;
use crate::skills::{self, Skill};
use crate::title::Titler;
use crate::transcript::{self, Image, Page, Tail, ToolOutput, Transcripts, Window};
use crate::turns::{Counts, Diff, Snapshot, Turns};
use crate::watch::{Changes, Watcher};

/// How long FSEvents waits for more changes to merge with one. pi writes a
/// line per message, so this only merges bursts, like a compaction's.
const WATCH_LATENCY: Duration = Duration::from_millis(100);

/// How long a prompt the chat's own pi may have written waits for pi to
/// report it, before it counts as another pi's. pi writes the file and
/// reports the message in either order.
const REPORT_WAIT: Duration = Duration::from_secs(1);

/// How many of its own prompts a chat remembers. Only the newest in the file
/// is ever looked up.
const SENT_KEPT: usize = 64;
/// Alerts waiting for a slow listener. Each is a moment that's passed, so
/// losing old ones is fine.
const ALERTS: usize = 64;

/// Where the engine keeps its files and finds pi's, and how it runs pi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The `pi` executable.
    pub pi: PathBuf,
    /// pi's sessions folder, usually `~/.pi/agent/sessions`.
    pub sessions_dir: PathBuf,
    /// The engine's own folder, usually `~/Library/Application Support/Ludolph`.
    pub state_dir: PathBuf,
    /// How long a settled chat keeps its pi.
    pub idle: Duration,
    /// How long pi gets to exit before it's killed.
    pub grace: Duration,
    /// The user's home folder. Chats there have no project.
    pub home: PathBuf,
    /// The Ludolph.app the engine runs from, which the login item opens.
    /// `None` outside one, as for a development engine, which leaves the
    /// login item alone.
    pub app: Option<PathBuf>,
    /// Where the relay token and the phones' secrets are kept.
    pub secrets: SecretStore,
}

impl Config {
    /// Where chats started without a project get their folders.
    pub fn scratch_dir(&self) -> PathBuf {
        self.state_dir.join("scratch")
    }

    /// Whether chats in `cwd` belong to a project, rather than to "No
    /// project" with the home and scratch folders.
    pub fn is_project(&self, cwd: &Path) -> bool {
        cwd != self.home && !cwd.starts_with(self.scratch_dir())
    }
}

/// The engine. Clones share it.
#[derive(Debug, Clone)]
pub struct Engine(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    config: Config,
    state: Mutex<State>,
    /// Bumped whenever the chat list changes.
    list: watch::Sender<u64>,
    alerts: broadcast::Sender<Alert>,
    /// Its own lock, since reading a transcript can take a while.
    transcripts: Arc<Mutex<Transcripts>>,
    /// The task that follows the sessions folder.
    watching: AbortHandle,
    /// Shared with titling tasks, which don't keep the engine alive.
    titler: Arc<Titler>,
    /// The saved settings. Writes hold the lock until the file is saved, so
    /// they land in order.
    settings: Mutex<Settings>,
    turns: Arc<Turns>,
    catalog: Catalog,
    /// The task that keeps the relay link, if this Mac has one.
    relay: Mutex<Option<AbortHandle>>,
    /// The linked phones' keys, which the relay link uses.
    phones: Arc<Phones>,
    phone: watch::Sender<Option<Phone>>,
    /// The task that waits for a phone to link, while Settings shows a code.
    pairing_task: Mutex<Option<AbortHandle>>,
    pairing: watch::Sender<Option<Pairing>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.watching.abort();
        if let Some(relay) = self.relay.get_mut().ok().and_then(Option::take) {
            relay.abort();
        }
        if let Some(pairing) = self.pairing_task.get_mut().ok().and_then(Option::take) {
            pairing.abort();
        }
    }
}

#[derive(Debug)]
struct State {
    index: Index,
    runtime: Runtime,
    chats: HashMap<String, Chat>,
    /// The engine is shutting down, so the runs it stops aren't news to
    /// tell anyone about.
    stopping: bool,
}

/// What the engine knows about a chat beyond its session file.
#[derive(Debug)]
struct Chat {
    /// Held by commands that start or stop the chat's pi, so they don't overlap.
    ops: Arc<tokio::sync::Mutex<()>>,
    /// A new chat whose session file pi hasn't written yet.
    draft: Option<Draft>,
    process: Option<Running>,
    /// Bumped by every stop, so a send that started before one doesn't go
    /// out after it.
    stops: u64,
    /// The `timestamp`s of the user messages the chat's own pi reported.
    sent: VecDeque<i64>,
    /// Another pi wrote a prompt to the session, so sending waits for a
    /// take-over.
    elsewhere: bool,
    /// The title the engine or you gave the chat, which is newer than the
    /// file's until pi writes it there.
    named: Option<String>,
    /// A title is being asked for.
    titling: bool,
    /// The run Ludolph started, from its snapshot until its changes are
    /// saved.
    run: Option<Run>,
    /// Bumped whenever the chat's view changes.
    changes: watch::Sender<u64>,
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            ops: Arc::default(),
            draft: None,
            process: None,
            stops: 0,
            sent: VecDeque::new(),
            elsewhere: false,
            named: None,
            titling: false,
            run: None,
            changes: watch::channel(0).0,
        }
    }
}

impl Chat {
    fn changed(&self) {
        self.changes.send_modify(|revision| *revision += 1);
    }

    /// Remember a user message the chat's own pi reported.
    fn sent(&mut self, message: &Value) {
        let Some(at) = message["timestamp"]
            .as_i64()
            .filter(|_| message["role"] == "user")
        else {
            return;
        };
        if !self.sent.contains(&at) {
            if self.sent.len() == SENT_KEPT {
                self.sent.pop_front();
            }
            self.sent.push_back(at);
        }
    }
}

/// A run whose changes will be saved once it settles.
#[derive(Debug)]
struct Run {
    before: Snapshot,
    /// The timestamp of the user message that started it, once pi reports
    /// it. Views find the run by it.
    key: Option<i64>,
    settled: bool,
}

#[derive(Debug, Clone)]
struct Draft {
    cwd: PathBuf,
    /// The file pi will write, from `get_state`.
    path: Option<PathBuf>,
    created: DateTime<Utc>,
}

#[derive(Debug)]
struct Running {
    pi: Arc<Pi>,
    live: Live,
    /// Its extensions' open dialogs, oldest first. They end with the process.
    questions: Vec<Question>,
}

/// An extension's dialog, waiting for an answer (`docs/rpc-extension-ui.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// pi's request id, which the answer names.
    pub id: String,
    pub dialog: Dialog,
    pub title: String,
    /// What a `confirm` asks about.
    pub message: Option<String>,
    /// A `select`'s choices.
    pub options: Vec<String>,
    /// An `editor`'s starting text.
    pub prefill: Option<String>,
}

/// What kind of answer a [`Question`] takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    /// One of its options.
    Select,
    /// Yes or no.
    Confirm,
    /// One line of text.
    Input,
    /// Text of any length.
    Editor,
}

/// An answer to a [`Question`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The picked option or typed text, for every dialog but `confirm`.
    Value(String),
    /// A `confirm`'s answer.
    Confirmed(bool),
}

/// A chat as the list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatInfo {
    /// pi's session id.
    pub id: String,
    pub cwd: PathBuf,
    pub created: DateTime<Utc>,
    /// The session's summary; `None` for a draft.
    pub summary: Option<Summary>,
    /// The session's name, or the title the engine just gave it.
    pub title: Option<String>,
    pub status: Status,
    /// Another pi wrote to the session, so sending is off until a take-over.
    pub elsewhere: bool,
}

/// A chat and the latest messages of its transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub chat: ChatInfo,
    pub page: Page,
    /// Changes whenever the page would, while the chat is working.
    pub revision: u64,
    /// The oldest dialog the chat's pi waits on.
    pub question: Option<Question>,
    /// What the chat's run waits on, while one is going.
    pub run: Option<live::Run>,
}

/// What a chat is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working,
    /// An extension's dialog waits for an answer.
    AwaitingInput,
    /// The last run ended on an error.
    Failed,
    /// pi or the engine stopped during the last run.
    Interrupted,
}

/// A moment in a chat worth telling you about while you're away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub chat: String,
    pub kind: AlertKind,
}

/// What happened in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// A run ended well. One you stopped yourself doesn't count.
    Finished,
    /// A run ended on an error.
    Failed,
    /// pi exited during a run.
    Interrupted,
    /// An extension's dialog waits for an answer.
    Input,
}

/// What pi did with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// It started a run.
    Started,
    /// It queued the message to steer the running one.
    Queued,
    /// An extension command or input handler took it, and no run started.
    Handled,
}

/// The model settings, context use and spend a chat's composer shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Composer {
    /// `provider/model`; `None` before pi has picked one.
    pub model: Option<String>,
    pub thinking: Option<String>,
    /// The tokens in the context; `None` right after a compaction.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub spent: Spent,
}

impl Engine {
    /// Start the engine: stop the pi processes a previous engine left
    /// running, bring the index up to date, and keep it so as pi writes.
    ///
    /// The sessions folder is created if it doesn't exist, so it can be
    /// watched.
    ///
    /// # Errors
    ///
    /// Fails if the engine's folder or the sessions folder can't be read or
    /// written.
    pub async fn start(config: Config) -> Result<Self> {
        std::fs::create_dir_all(&config.state_dir)
            .with_context(|| format!("creating {}", config.state_dir.display()))?;
        let runtime_path = config.state_dir.join("runtime.json");
        let (runtime, interrupted) = Runtime::reap(&runtime_path, config.grace).await?;
        std::fs::create_dir_all(&config.sessions_dir)
            .with_context(|| format!("creating {}", config.sessions_dir.display()))?;
        // Watching starts first, so writes during the refresh aren't missed.
        let watcher = Watcher::start(&config.sessions_dir, WATCH_LATENCY)?;
        let index_path = config.state_dir.join("index.json");
        let sessions_dir = config.sessions_dir.clone();
        let index = tokio::task::spawn_blocking(move || -> Result<Index> {
            // The index is rebuilt from the session files, so a broken one
            // only costs time.
            let mut index = Index::load(&index_path).unwrap_or_else(|e| {
                eprintln!("rebuilding the index: {e:#}");
                Index::default()
            });
            index.refresh(&sessions_dir)?;
            for id in &interrupted {
                index.set_interrupted(id);
            }
            index.save(&index_path)?;
            Ok(index)
        })
        .await??;
        let state = State {
            index,
            runtime,
            chats: HashMap::new(),
            stopping: false,
        };
        let settings = Settings::load(&config.state_dir)?;
        // A moved app moves its login item.
        if let Err(e) = login::apply(&config.home, config.app.as_deref(), settings.open_at_login) {
            eprintln!("login item: {e:#}");
        }
        let turns = Arc::new(Turns::new(config.state_dir.join("turns"))?);
        // The engine's folder has no `.pi` settings to change the list.
        let catalog = Catalog::new(config.pi.clone(), config.state_dir.clone(), config.grace);
        let engine = Self(Arc::new_cyclic(|inner| Inner {
            turns,
            catalog,
            state: Mutex::new(state),
            list: watch::channel(0).0,
            alerts: broadcast::channel(ALERTS).0,
            transcripts: Arc::default(),
            watching: tokio::spawn(follow(inner.clone(), watcher)).abort_handle(),
            titler: Arc::new(Titler::new(config.pi.clone())),
            settings: Mutex::new(settings),
            relay: Mutex::default(),
            phones: Arc::default(),
            phone: watch::channel(None).0,
            pairing_task: Mutex::default(),
            pairing: watch::channel(None).0,
            config,
        }));
        tokio::spawn(engine.clone().reconnect());
        Ok(engine)
    }

    /// Every chat, most recently active first.
    pub fn chats(&self) -> Vec<ChatInfo> {
        let state = self.state();
        let mut chats: Vec<ChatInfo> = state
            .index
            .chats()
            .map(|(_, summary)| info(&state, summary))
            .collect();
        let drafts = state
            .chats
            .iter()
            .filter_map(|(id, chat)| draft_info(&state, id, chat));
        chats.extend(drafts.collect::<Vec<_>>());
        chats.sort_by_key(|chat| {
            let active = chat.summary.as_ref().and_then(|s| s.last_activity);
            std::cmp::Reverse(active.unwrap_or(chat.created))
        });
        chats
    }

    /// A counter that changes whenever [`Engine::chats`] would.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.list.subscribe()
    }

    /// The chats' alerts from now on. A receiver that falls far behind
    /// loses the oldest ones.
    pub fn alerts(&self) -> broadcast::Receiver<Alert> {
        self.0.alerts.subscribe()
    }

    /// The details of tool call `call` in chat `id`: what it printed so far
    /// while it runs, then its whole result.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat or call, or the file can't be read.
    pub async fn tool_output(&self, id: &str, call: &str) -> Result<ToolOutput> {
        let path = {
            let state = self.state();
            let chat = state.chats.get(id);
            let running = chat.and_then(|chat| chat.process.as_ref());
            if let Some(tool) = running.and_then(|r| r.live.tools().get(call)) {
                return Ok(transcript::running_tool(tool));
            }
            file_of(&state.index, id, chat.and_then(|chat| chat.draft.as_ref()))
        };
        let path = path.with_context(|| format!("no chat {id}"))?;
        let transcripts = self.0.transcripts.clone();
        let call_id = call.to_owned();
        let found = tokio::task::spawn_blocking(move || {
            transcripts
                .lock()
                .expect("lock poisoned")
                .tool(&path, &call_id)
        })
        .await??;
        found.with_context(|| format!("no tool call {call} in chat {id}"))
    }

    /// The `index`th image of message `message` in chat `id`.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat or image, or the file can't be read.
    pub async fn image(&self, id: &str, message: &str, index: usize) -> Result<Image> {
        let path = {
            let state = self.state();
            let draft = state.chats.get(id).and_then(|chat| chat.draft.as_ref());
            file_of(&state.index, id, draft)
        };
        let path = path.with_context(|| format!("no chat {id}"))?;
        let transcripts = self.0.transcripts.clone();
        let entry = message.to_owned();
        let found = tokio::task::spawn_blocking(move || {
            transcripts
                .lock()
                .expect("lock poisoned")
                .image(&path, &entry, index)
        })
        .await??;
        found.with_context(|| format!("no image {index} in message {message}"))
    }

    /// A chat with the `window` of its transcript.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat, or its session file can't be read.
    pub async fn view(&self, id: &str, window: Window) -> Result<View> {
        let (chat, path, messages, streaming, revision, question, run) = {
            let state = self.state();
            let chat = state.chats.get(id);
            let info = match state.index.find(id) {
                Some((_, summary)) => info(&state, summary),
                None => chat
                    .and_then(|chat| draft_info(&state, id, chat))
                    .with_context(|| format!("no chat {id}"))?,
            };
            let path = chat.map_or_else(
                || file_of(&state.index, id, None),
                |chat| file_of(&state.index, id, chat.draft.as_ref()),
            );
            let running = chat.and_then(|chat| chat.process.as_ref());
            let question = running.and_then(|r| r.questions.first().cloned());
            let live = running.map(|r| &r.live);
            // Copied, so the transcript is read without holding the state.
            let messages = live
                .map(|live| live.messages().to_vec())
                .unwrap_or_default();
            let streaming = live.and_then(|live| live.streaming().cloned());
            let run = live.and_then(Live::run);
            let revision = chat.map_or(0, |chat| *chat.changes.borrow());
            (info, path, messages, streaming, revision, question, run)
        };
        let transcripts = self.0.transcripts.clone();
        let page = tokio::task::spawn_blocking(move || {
            let tail = Tail {
                messages: &messages,
                streaming: streaming.as_ref(),
            };
            let mut transcripts = transcripts.lock().expect("lock poisoned");
            transcripts.page(path.as_deref().filter(|p| p.exists()), tail, &window)
        })
        .await??;
        Ok(View {
            chat,
            page,
            revision,
            question,
            run,
        })
    }

    /// A counter that changes whenever [`Engine::view`] would for chat `id`.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat.
    pub fn subscribe_chat(&self, id: &str) -> Result<watch::Receiver<u64>> {
        let mut state = self.state();
        known(&state, id)?;
        Ok(state
            .chats
            .entry(id.to_owned())
            .or_default()
            .changes
            .subscribe())
    }

    /// The engine's settings.
    pub fn config(&self) -> &Config {
        &self.0.config
    }

    /// Start a chat in `cwd`, and return its id. It's a draft until its first
    /// message, since pi writes the session file only then.
    ///
    /// # Errors
    ///
    /// Fails if `cwd` isn't a folder, or pi can't be started.
    pub async fn create(&self, cwd: &Path) -> Result<String> {
        if !cwd.is_dir() {
            bail!("{} isn't a folder", cwd.display());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let draft = Draft {
            cwd: cwd.to_owned(),
            path: None,
            created: Utc::now(),
        };
        let ops = {
            let mut state = self.state();
            let chat = state.chats.entry(id.clone()).or_default();
            chat.draft = Some(draft);
            chat.ops.clone()
        };
        let _ops = ops.lock().await;
        let pi = self.process(&id)?;
        let session = pi.call(json!({"type": "get_state"})).await?;
        let path = session["sessionFile"]
            .as_str()
            .context("pi didn't name the new chat's session file")?;
        let mut state = self.state();
        if let Some(draft) = state
            .chats
            .get_mut(&id)
            .and_then(|chat| chat.draft.as_mut())
        {
            draft.path = Some(PathBuf::from(path));
        }
        drop(state);
        self.changed();
        Ok(id)
    }

    /// Send `text` to a chat, with images. It starts a run, or steers the
    /// one that's going.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat, its folder is gone, or pi refuses the
    /// message.
    pub async fn send(&self, id: &str, text: &str, images: Vec<Image>) -> Result<Delivery> {
        let ops = self.ops(id)?;
        let stops = self.stops(id);
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let pi = self.process(id)?;
        // The last run's changes are taken before anything can change more.
        self.finish_run(id).await;
        let (working, cwd) = {
            let state = self.state();
            let chat = state.chats.get(id).expect("a chat with a process is known");
            let working = chat.process.as_ref().is_some_and(|r| r.live.working());
            (working, cwd_of(&state, id))
        };
        // A message to an idle chat starts a run, so the tree is taken first.
        let before = match cwd.filter(|_| !working) {
            Some(cwd) => self.0.turns.before(&cwd).await.unwrap_or_else(|e| {
                eprintln!("chat {id}: skipping this run's changes: {e:#}");
                None
            }),
            None => None,
        };
        let snapshotted = before.is_some();
        {
            let mut state = self.state();
            let chat = state
                .chats
                .get_mut(id)
                .expect("a chat with a process is known");
            if let Some(before) = before {
                chat.run = Some(Run {
                    before,
                    key: None,
                    settled: false,
                });
            }
        }
        let settled = self.live_of(id, &pi, |live| live.settled());
        // Stop can come while pi starts or the tree is taken, before there's
        // a run to abort.
        let delivery = if self.stops(id) == stops {
            self.prompt(&pi, &skills::to_front(text), images).await
        } else {
            Err(anyhow!("stopped before it was sent"))
        };
        if let (Ok(Delivery::Started), Some(settled)) = (&delivery, settled)
            && self.live_of(id, &pi, |live| live.started(settled)) == Some(true)
        {
            if let Some(chat) = self.state().chats.get(id) {
                chat.changed();
            }
            self.changed();
        }
        if snapshotted
            && !matches!(delivery, Ok(Delivery::Started))
            && let Some(chat) = self.state().chats.get_mut(id)
        {
            chat.run = None;
        }
        let delivery = delivery?;
        if delivery != Delivery::Handled {
            self.title_soon(id, text);
        }
        Ok(delivery)
    }

    /// Run `f` on the live tail of chat `id`, if `pi` is still its process.
    fn live_of<T>(&self, id: &str, pi: &Arc<Pi>, f: impl FnOnce(&mut Live) -> T) -> Option<T> {
        let mut state = self.state();
        let running = state.chats.get_mut(id)?.process.as_mut()?;
        Arc::ptr_eq(&running.pi, pi).then(|| f(&mut running.live))
    }

    /// Send `text` to `pi` as a prompt, and return what pi did with it.
    async fn prompt(&self, pi: &Pi, text: &str, images: Vec<Image>) -> Result<Delivery> {
        // pi ignores `steer` when no run is going, so the answer says which
        // happened, even if a run started or ended meanwhile.
        let mut command = json!({"type": "prompt", "message": text, "streamingBehavior": "steer"});
        if !images.is_empty() {
            let images = images.into_iter().map(
                |image| json!({"type": "image", "mimeType": image.mime_type, "data": image.data}),
            );
            command["images"] = Value::Array(images.collect());
        }
        let answer = pi.call(command).await?;
        match answer["disposition"].as_str() {
            Some("started") => Ok(Delivery::Started),
            Some("queued") => Ok(Delivery::Queued),
            Some("handled") => Ok(Delivery::Handled),
            other => bail!("pi answered the prompt with {other:?}"),
        }
    }

    /// Save the changes of the chat's run if it settled. Callers hold the
    /// chat's `ops`, so a new run can't start before its snapshot.
    async fn finish_run(&self, id: &str) {
        let run = {
            let mut state = self.state();
            let chat = state.chats.get_mut(id);
            chat.and_then(|chat| chat.run.take_if(|run| run.settled))
        };
        let Some(Run {
            before,
            key: Some(key),
            ..
        }) = run
        else {
            return;
        };
        let saved = async {
            let changes = self.0.turns.after(before).await?;
            let (turns, session) = (self.0.turns.clone(), id.to_owned());
            tokio::task::spawn_blocking(move || turns.save(&session, key, &changes)).await?
        };
        match saved.await {
            Ok(()) => {
                if let Some(chat) = self.state().chats.get(id) {
                    chat.changed();
                }
            }
            Err(e) => eprintln!("chat {id}: saving this run's changes: {e:#}"),
        }
    }

    /// The sizes of chat `id`'s saved runs, by the timestamp of the user
    /// message that started each.
    ///
    /// # Errors
    ///
    /// Fails if they can't be read.
    pub async fn run_counts(&self, id: &str) -> Result<BTreeMap<i64, Counts>> {
        let (turns, id) = (self.0.turns.clone(), id.to_owned());
        tokio::task::spawn_blocking(move || turns.counts(&id)).await?
    }

    /// What chat `id`'s run `key` changed, if it was saved.
    ///
    /// # Errors
    ///
    /// Fails if it can't be read.
    pub async fn run_changes(&self, id: &str, key: i64) -> Result<Option<Diff>> {
        let (turns, id) = (self.0.turns.clone(), id.to_owned());
        tokio::task::spawn_blocking(move || turns.load(&id, key)).await?
    }

    /// Ask for a title in the background if the chat has none, from its
    /// first prompt or else from `text`, the message just sent.
    fn title_soon(&self, id: &str, text: &str) {
        let prompt = {
            let mut state = self.state();
            let summary = state.index.find(id).map(|(_, s)| s);
            if summary.is_some_and(|s| s.title.is_some()) {
                return;
            }
            let prompt = summary.and_then(|s| s.first_prompt.clone());
            let prompt = prompt.unwrap_or_else(|| text.to_owned());
            // An image alone gives a title nothing to go on; the next text will.
            if prompt.trim().is_empty() {
                return;
            }
            let Some(chat) = state.chats.get_mut(id) else {
                return;
            };
            if chat.named.is_some() || chat.titling {
                return;
            }
            chat.titling = true;
            prompt
        };
        let engine = Arc::downgrade(&self.0);
        let titler = self.0.titler.clone();
        let model = self.settings().sidecar_model;
        let id = id.to_owned();
        tokio::spawn(async move {
            let title = titler.title(&prompt, model.as_deref()).await;
            let Some(engine) = engine.upgrade().map(Engine) else {
                return;
            };
            let named = match title {
                Ok(title) => engine.name(&id, &title).await,
                Err(e) => Err(e),
            };
            if let Some(chat) = engine.state().chats.get_mut(&id) {
                chat.titling = false;
            }
            if let Err(e) = named {
                eprintln!("titling chat {id}: {e:#}");
            }
        });
    }

    /// Give chat `id` the title `title` through its pi, unless it got one
    /// meanwhile. A chat whose pi has stopped is left for its next message.
    async fn name(&self, id: &str, title: &str) -> Result<()> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        let pi = {
            let state = self.state();
            let running = state.chats.get(id).and_then(|chat| chat.process.as_ref());
            running.map(|running| running.pi.clone())
        };
        let Some(pi) = pi else {
            return Ok(());
        };
        let session = pi.call(json!({"type": "get_state"})).await?;
        if session["sessionName"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
        {
            return Ok(());
        }
        pi.call(json!({"type": "set_session_name", "name": title}))
            .await?;
        let mut state = self.state();
        if let Some(chat) = state.chats.get_mut(id) {
            chat.named = Some(title.to_owned());
            chat.changed();
        }
        drop(state);
        self.changed();
        Ok(())
    }

    /// Rename chat `id` to `title` through its pi, which it starts if it has
    /// to, since pi owns the session file.
    ///
    /// # Errors
    ///
    /// Fails if the title is empty, there's no such chat, another pi has it
    /// open, or pi refuses.
    pub async fn rename(&self, id: &str, title: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            bail!("the title is empty");
        }
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let pi = self.process(id)?;
        pi.call(json!({"type": "set_session_name", "name": title}))
            .await?;
        let mut state = self.state();
        if let Some(chat) = state.chats.get_mut(id) {
            chat.named = Some(title.to_owned());
            chat.changed();
        }
        drop(state);
        self.changed();
        Ok(())
    }

    /// Delete chat `id`: stop its pi, then remove every copy of its
    /// session file and its saved turn changes.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat, another pi has it open, its pi can't
    /// be stopped, or a file can't be removed.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        // Taken first, so the stop doesn't count as an interrupted run.
        let running = self
            .state()
            .chats
            .get_mut(id)
            .and_then(|chat| chat.process.take());
        if let Some(running) = running {
            running.pi.stop(self.0.config.grace).await?;
        }
        let files: Vec<PathBuf> = {
            let state = self.state();
            let draft = state.chats.get(id).and_then(|chat| chat.draft.as_ref());
            let copies = state.index.iter().filter(|(_, summary)| summary.id == id);
            copies
                .map(|(path, _)| path.to_owned())
                .chain(draft.and_then(|draft| draft.path.clone()))
                .collect()
        };
        for file in &files {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("removing {}", file.display()));
                }
                _ => {}
            }
        }
        {
            let mut state = self.state();
            for file in &files {
                state.index.refresh_file(file)?;
            }
            state.chats.remove(id);
        }
        self.0.turns.forget(id)?;
        self.save_index()?;
        self.changed();
        Ok(())
    }

    /// Turn sending back on for a chat another pi wrote to. The next message
    /// starts a fresh pi, which loads the file as it is then.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat.
    pub async fn take_over(&self, id: &str) -> Result<()> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        if let Some(chat) = self.state().chats.get_mut(id) {
            chat.elsewhere = false;
            chat.changed();
        }
        self.changed();
        Ok(())
    }

    /// `heard` from dictation, cleaned up by the sidecar model.
    ///
    /// # Errors
    ///
    /// Fails if it's blank, or the model fails.
    pub async fn polish(&self, heard: &str) -> Result<String> {
        let model = self.settings().sidecar_model;
        self.0.titler.polish(heard, model.as_deref()).await
    }

    /// The current settings.
    pub fn settings(&self) -> Settings {
        self.0
            .settings
            .lock()
            .expect("the settings lock isn't poisoned")
            .clone()
    }

    /// Save `settings` and use them from now on.
    ///
    /// # Errors
    ///
    /// Fails if the sidecar model isn't one pi lists, or the file can't be
    /// written.
    pub async fn set_settings(&self, settings: Settings) -> Result<Settings> {
        if let Some(model) = &settings.sidecar_model
            && self.0.catalog.find(model).await?.is_none()
        {
            bail!("pi has no model {model}");
        }
        let config = &self.0.config;
        login::apply(&config.home, config.app.as_deref(), settings.open_at_login)?;
        let mut saved = self
            .0
            .settings
            .lock()
            .expect("the settings lock isn't poisoned");
        settings.save(&self.0.config.state_dir)?;
        *saved = settings.clone();
        Ok(settings)
    }

    /// How this Mac's link to the relay stands, if it has one.
    pub fn phone(&self) -> Option<Phone> {
        self.0.phone.borrow().clone()
    }

    /// A phone linking through this Mac, if one is or just did.
    pub fn pairing(&self) -> Option<Pairing> {
        self.0.pairing.borrow().clone()
    }

    /// The relay this Mac links through: its link's, or the default.
    pub fn relay(&self) -> String {
        self.phone()
            .map_or_else(|| DEFAULT_RELAY.to_owned(), |phone| phone.relay)
    }

    /// Start waiting for a phone to link through `relay`, or through the
    /// relay this Mac is linked to, in place of any wait under way. A Mac new
    /// to the relay keeps the link the relay gives it.
    ///
    /// # Errors
    ///
    /// Fails if the address isn't a relay's, or names another relay than
    /// the one this Mac is linked to.
    pub fn link_phone(&self, relay: Option<&str>) -> Result<()> {
        let saved = Link::load(&self.0.config.state_dir, self.0.config.secrets)?;
        let relay = match (&saved, relay.map(|r| r.trim().trim_end_matches('/'))) {
            (Some(link), Some(other)) if other != link.relay => {
                bail!("this Mac is linked to {}; unlink it first", link.relay)
            }
            (Some(link), _) => link.relay.clone(),
            (None, relay) => relay.unwrap_or(DEFAULT_RELAY).to_owned(),
        };
        relay::socket_url(&relay, "")?;
        self.stop_pairing();
        self.0.pairing.send_replace(Some(Pairing::Starting));
        let engine = self.clone();
        let task = tokio::spawn(async move {
            let token = saved.map(|link| link.token);
            let state = match engine.pair(&relay, token).await {
                Ok(()) => Pairing::Linked,
                Err(e) => {
                    eprintln!("relay: linking a phone: {e:#}");
                    Pairing::Failed {
                        error: e.root_cause().to_string(),
                    }
                }
            };
            engine.0.pairing.send_replace(Some(state));
        });
        *self
            .0
            .pairing_task
            .lock()
            .expect("the pairing lock isn't poisoned") = Some(task.abort_handle());
        Ok(())
    }

    /// Wait on `relay` for a phone to link, and keep its secret, and the link
    /// a new Mac gets.
    async fn pair(&self, relay: &str, token: Option<String>) -> Result<()> {
        let secret = seal::new_secret()?;
        let shown = |url, expires_in: u64| {
            let expires_in = i64::try_from(expires_in).unwrap_or(i64::MAX / 2_000);
            let expires_at = chrono::Utc::now().timestamp_millis() + expires_in * 1000;
            self.0
                .pairing
                .send_replace(Some(Pairing::Waiting { url, expires_at }));
        };
        let token = match relay::pair(relay, token.as_deref(), &secret, shown).await {
            // The relay forgot this Mac, so it links as a new one.
            Err(e) if token.is_some() && relay::is_rejected(&e) => {
                relay::pair(relay, None, &secret, shown).await?
            }
            other => other?,
        };
        let config = &self.0.config;
        match token {
            Some(token) => {
                let link = Link {
                    relay: relay.to_owned(),
                    token,
                    phones: vec![secret],
                };
                link.save(&config.state_dir, config.secrets)?;
                self.connect(link)?;
            }
            None => {
                let mut link = Link::load(&config.state_dir, config.secrets)?
                    .context("this Mac was unlinked while a phone linked")?;
                link.phones.push(secret.clone());
                link.save(&config.state_dir, config.secrets)?;
                self.0.phones.add(&secret)?;
            }
        }
        Ok(())
    }

    /// Stop waiting for a phone to link, and drop how the last wait ended.
    pub fn stop_pairing(&self) {
        let task = self
            .0
            .pairing_task
            .lock()
            .expect("the pairing lock isn't poisoned")
            .take();
        if let Some(task) = task {
            task.abort();
        }
        self.0.pairing.send_replace(None);
    }

    /// Disconnect from the relay, forget the link, and have the relay
    /// forget this Mac in the background.
    ///
    /// # Errors
    ///
    /// Fails if the saved link can't be read or removed.
    pub fn unlink_phone(&self) -> Result<()> {
        self.stop_pairing();
        let config = &self.0.config;
        let link = Link::load(&config.state_dir, config.secrets)?;
        self.disconnect();
        self.0.phones.set(&[])?;
        Link::remove(&config.state_dir, config.secrets)?;
        if let Some(link) = link {
            // The phone shows a Mac the relay keeps as offline until it's removed there.
            tokio::spawn(async move {
                if let Err(e) = relay::forget(&link).await {
                    eprintln!("relay: the relay may still list this Mac: {e:#}");
                }
            });
        }
        Ok(())
    }

    /// Connect the saved link, if there is one. macOS may ask the user before
    /// it gives out the Keychain item, say after the app's signature changed,
    /// so the engine serves the app meanwhile, and a refusal leaves it
    /// unlinked.
    async fn reconnect(self) {
        let (dir, store) = (self.0.config.state_dir.clone(), self.0.config.secrets);
        let loaded = tokio::task::spawn_blocking(move || Link::load(&dir, store)).await;
        let connected = match loaded {
            Ok(Ok(Some(link))) => self.connect(link),
            Ok(Ok(None)) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(e.into()),
        };
        if let Err(e) = connected {
            eprintln!("relay: not connecting the saved link: {e:#}");
        }
    }

    /// Keep `link` to the relay from now on, in place of any other.
    fn connect(&self, link: Link) -> Result<()> {
        self.disconnect();
        self.0.phones.set(&link.phones)?;
        self.0.phone.send_replace(Some(Phone {
            relay: link.relay.clone(),
            status: LinkStatus::Connecting,
            error: None,
        }));
        let run = relay::run(
            self.clone(),
            link,
            self.0.phones.clone(),
            self.0.phone.clone(),
        );
        let task = tokio::spawn(run);
        *self.0.relay.lock().expect("the relay lock isn't poisoned") = Some(task.abort_handle());
        Ok(())
    }

    fn disconnect(&self) {
        let task = self
            .0
            .relay
            .lock()
            .expect("the relay lock isn't poisoned")
            .take();
        if let Some(task) = task {
            task.abort();
        }
        self.0.phone.send_replace(None);
    }

    /// The models pi can use.
    ///
    /// # Errors
    ///
    /// Fails if pi can't list them.
    pub async fn models(&self) -> Result<Arc<Vec<Model>>> {
        self.0.catalog.models().await
    }

    /// The model pi starts a chat on when none is picked, if it has one.
    ///
    /// # Errors
    ///
    /// Fails if pi can't say.
    pub async fn default_model(&self) -> Result<Option<String>> {
        self.0.catalog.default_model().await
    }

    /// Switch chat `id` to `model` (`provider/model`) and the thinking level
    /// `thinking` from its next answer, and return its settings then. pi
    /// keeps the level within what the model supports, and records both in
    /// the session file.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat, another pi has it open, or pi refuses.
    pub async fn set_model(
        &self,
        id: &str,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<Composer> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let pi = self.process(id)?;
        if let Some(model) = model {
            let (provider, model) = model
                .split_once('/')
                .with_context(|| format!("model {model} isn't provider/model"))?;
            let command = json!({"type": "set_model", "provider": provider, "modelId": model});
            pi.call(command).await?;
        }
        if let Some(level) = thinking {
            pi.call(json!({"type": "set_thinking_level", "level": level}))
                .await?;
        }
        live_composer(&pi).await
    }

    /// Chat `id`'s model, thinking level and context use: its pi's if it's
    /// running, otherwise what its session file last recorded, so asking
    /// doesn't start pi.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat.
    pub async fn composer(&self, id: &str) -> Result<Composer> {
        let (pi, saved) = {
            let state = self.state();
            known(&state, id)?;
            let running = state.chats.get(id).and_then(|chat| chat.process.as_ref());
            let saved = state.index.find(id).map(|(_, summary)| Composer {
                model: summary.model.clone(),
                thinking: summary.thinking.clone(),
                context_tokens: summary.context_tokens,
                context_window: None,
                spent: summary.spent,
            });
            (running.map(|running| running.pi.clone()), saved)
        };
        if let Some(pi) = pi {
            match live_composer(&pi).await {
                Ok(composer) => return Ok(composer),
                // It may be stopping for being idle; the file has the same.
                Err(e) => eprintln!("chat {id}: asking pi for its model: {e:#}"),
            }
        }
        let mut composer = saved.unwrap_or_default();
        if let Some(model) = &composer.model {
            let found = self.0.catalog.find(model).await.unwrap_or_else(|e| {
                eprintln!("chat {id}: finding its context window: {e:#}");
                None
            });
            composer.context_window = found.and_then(|model| model.context_window);
        }
        Ok(composer)
    }

    /// The skills chat `id` can run: its pi's if it's running, otherwise
    /// those a pi in its folder would load.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat, or pi can't list them.
    pub async fn skills(&self, id: &str) -> Result<Vec<Skill>> {
        let (pi, cwd) = {
            let state = self.state();
            known(&state, id)?;
            let running = state.chats.get(id).and_then(|chat| chat.process.as_ref());
            (
                running.map(|running| running.pi.clone()),
                cwd_of(&state, id),
            )
        };
        if let Some(pi) = pi {
            match pi.call(json!({"type": "get_commands"})).await {
                Ok(answer) => return Ok(skills::from_commands(&answer)),
                // It may be stopping for being idle.
                Err(e) => eprintln!("chat {id}: asking pi for its skills: {e:#}"),
            }
        }
        let cwd = cwd.with_context(|| format!("no chat {id}"))?;
        self.skills_in(Some(&cwd)).await
    }

    /// The skills a new chat in `cwd` could run, or in a scratch folder
    /// without one.
    ///
    /// # Errors
    ///
    /// Fails if `cwd` isn't a folder, or pi can't list them.
    pub async fn skills_in(&self, cwd: Option<&Path>) -> Result<Vec<Skill>> {
        // A scratch folder has no skills of its own, like the engine's.
        let cwd = cwd.unwrap_or(&self.0.config.state_dir);
        if !cwd.is_dir() {
            bail!("{} isn't a folder", cwd.display());
        }
        let command = json!({"type": "get_commands"});
        let config = &self.0.config;
        let answer = Pi::ask(&config.pi, cwd, config.grace, command).await?;
        Ok(skills::from_commands(&answer))
    }

    /// The oldest dialog chat `id`'s pi waits on.
    pub fn question(&self, id: &str) -> Option<Question> {
        let state = self.state();
        let running = state.chats.get(id)?.process.as_ref()?;
        running.questions.first().cloned()
    }

    /// Answer the open dialog `question` in chat `id`. A question is answered
    /// once.
    ///
    /// # Errors
    ///
    /// Fails if the chat's pi has no such open question, the reply doesn't
    /// fit its dialog, or pi can't be written to.
    pub async fn respond(&self, id: &str, question: &str, reply: Reply) -> Result<()> {
        let pi = {
            let mut state = self.state();
            let running = state
                .chats
                .get_mut(id)
                .and_then(|chat| chat.process.as_mut());
            let Some(running) = running else {
                bail!("chat {id} has no open question {question}");
            };
            let Some(at) = running.questions.iter().position(|q| q.id == question) else {
                bail!("chat {id} has no open question {question}");
            };
            let dialog = running.questions[at].dialog;
            if matches!(reply, Reply::Confirmed(_)) != (dialog == Dialog::Confirm) {
                bail!("a {dialog:?} question can't take {reply:?}");
            }
            running.questions.remove(at);
            running.pi.clone()
        };
        self.asked(id);
        let mut answer = json!({"type": "extension_ui_response", "id": question});
        match reply {
            Reply::Value(value) => answer["value"] = value.into(),
            Reply::Confirmed(confirmed) => answer["confirmed"] = confirmed.into(),
        }
        pi.send(&answer).await
    }

    /// Stop a chat's run: dismiss its open dialogs, drop its queued
    /// messages, then abort.
    ///
    /// # Errors
    ///
    /// Fails if pi refuses.
    pub async fn interrupt(&self, id: &str) -> Result<()> {
        let (pi, questions) = {
            let mut state = self.state();
            let chat = state.chats.get_mut(id);
            let running = chat.and_then(|chat| {
                chat.stops += 1;
                chat.process.as_mut()
            });
            match running {
                Some(running) => (running.pi.clone(), std::mem::take(&mut running.questions)),
                None => return Ok(()),
            }
        };
        if !questions.is_empty() {
            self.asked(id);
        }
        for question in questions {
            let cancel =
                json!({"type": "extension_ui_response", "id": question.id, "cancelled": true});
            pi.send(&cancel).await?;
        }
        pi.call(json!({"type": "clear_queue"})).await?;
        pi.call(json!({"type": "abort"})).await?;
        Ok(())
    }

    /// Stop every pi, giving each the grace period to finish, and save the
    /// index.
    ///
    /// # Errors
    ///
    /// Fails if the index can't be saved.
    pub async fn shutdown(&self) -> Result<()> {
        self.0.watching.abort();
        self.stop_pairing();
        self.disconnect();
        let (processes, working): (Vec<Arc<Pi>>, Vec<String>) = {
            let mut state = self.state();
            state.stopping = true;
            let running = state
                .chats
                .iter()
                .filter_map(|(id, chat)| Some((id, chat.process.as_ref()?)));
            let working = running.clone().filter(|(_, r)| r.live.working());
            let working = working.map(|(id, _)| id.clone()).collect();
            (running.map(|(_, r)| r.pi.clone()).collect(), working)
        };
        let grace = self.0.config.grace;
        let stops: Vec<_> = processes
            .into_iter()
            .map(|pi| tokio::spawn(async move { pi.stop(grace).await }))
            .collect();
        for stop in stops {
            if let Err(e) = stop.await? {
                eprintln!("stopping pi: {e:#}");
            }
        }
        // Marked after pi's last write, its abort, which would end the mark.
        {
            let mut state = self.state();
            let State { index, chats, .. } = &mut *state;
            for id in working {
                let draft = chats.get(&id).and_then(|chat| chat.draft.as_ref());
                if let Some(path) = file_of(index, &id, draft) {
                    index.refresh_file(&path)?;
                }
                index.set_interrupted(&id);
            }
        }
        self.save_index()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().expect("lock poisoned")
    }

    /// Fail if another pi has chat `id` open, since two writers would split
    /// its session.
    fn refuse_elsewhere(&self, id: &str) -> Result<()> {
        let state = self.state();
        if state.chats.get(id).is_some_and(|chat| chat.elsewhere) {
            bail!("this chat is open in another pi; take it over to change it from Ludolph");
        }
        Ok(())
    }

    /// The lock that serializes a chat's commands.
    fn ops(&self, id: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let mut state = self.state();
        known(&state, id)?;
        Ok(state.chats.entry(id.to_owned()).or_default().ops.clone())
    }

    /// How many times chat `id` was stopped.
    fn stops(&self, id: &str) -> u64 {
        self.state().chats.get(id).map_or(0, |chat| chat.stops)
    }

    /// The chat's pi, started if it has none. Callers hold the chat's `ops`.
    fn process(&self, id: &str) -> Result<Arc<Pi>> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
            stopping,
        } = &mut *state;
        let chat = chats.get_mut(id).expect("callers look the chat up first");
        if let Some(running) = &chat.process {
            return Ok(running.pi.clone());
        }
        // Shutdown stops the processes it saw, so a later one would outlive it.
        if *stopping {
            bail!("the engine is shutting down");
        }
        let (cwd, session) = match (index.find(id), &chat.draft) {
            (Some((path, summary)), _) => (summary.cwd.clone(), Session::Resume(path.to_owned())),
            (None, Some(draft)) => match &draft.path {
                Some(path) if path.exists() => (draft.cwd.clone(), Session::Resume(path.clone())),
                _ => (draft.cwd.clone(), Session::New(id.to_owned())),
            },
            (None, None) => bail!("no chat {id}"),
        };
        if !cwd.is_dir() {
            bail!("this chat's folder, {}, no longer exists", cwd.display());
        }
        let options = Options {
            program: self.0.config.pi.clone(),
            cwd,
            session,
            extensions: Vec::new(),
            env: Vec::new(),
        };
        let (pi, events) = Pi::start(&options)?;
        let pi = Arc::new(pi);
        runtime.add(id, pi.pid())?;
        chat.process = Some(Running {
            pi: pi.clone(),
            live: Live::new(),
            questions: Vec::new(),
        });
        tokio::spawn(self.clone().watch(id.to_owned(), pi.clone(), events));
        Ok(pi)
    }

    /// Follow one pi process until it exits: fold its events in, and stop it
    /// once it has been idle for the configured time.
    async fn watch(self, id: String, pi: Arc<Pi>, mut events: mpsc::Receiver<Event>) {
        let idle = self.0.config.idle;
        let mut deadline = Some(Instant::now() + idle);
        loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        Event::AgentStart => deadline = None,
                        Event::AgentSettled { .. } => deadline = Some(Instant::now() + idle),
                        _ => {}
                    }
                    if let Err(e) = self.apply(&id, &pi, &event) {
                        eprintln!("chat {id}: {e:#}");
                    }
                }
                () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    deadline = match self.stop_idle(&id, &pi).await {
                        Ok(true) => None,
                        Ok(false) => Some(Instant::now() + idle),
                        Err(e) => {
                            eprintln!("chat {id}: stopping idle pi: {e:#}");
                            None
                        }
                    };
                }
            }
        }
        if let Err(e) = self.exited(&id, &pi) {
            eprintln!("chat {id}: {e:#}");
        }
    }

    /// Fold one of `pi`'s events into its chat.
    fn apply(&self, id: &str, pi: &Arc<Pi>, event: &Event) -> Result<()> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
            ..
        } = &mut *state;
        let Some(chat) = chats.get_mut(id) else {
            return Ok(());
        };
        if !chat
            .process
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(&r.pi, pi))
        {
            return Ok(());
        }
        if let Event::MessageStart { message } | Event::MessageEnd { message } = event {
            chat.sent(message);
            if let Some(run) = chat.run.as_mut().filter(|run| run.key.is_none())
                && message["role"] == "user"
            {
                run.key = message["timestamp"].as_i64();
            }
        }
        let mut finish = false;
        let mut alert = None;
        let running = chat.process.as_mut().expect("checked above");
        let working = running.live.working();
        let mut view_changed = running.live.apply(event);
        let mut list_changed = running.live.working() != working;
        match event {
            Event::AgentStart => runtime.set_running(id, true)?,
            Event::AgentSettled { aborted } => {
                if !aborted {
                    let failed = running.live.failure().is_some();
                    alert = Some(if failed {
                        AlertKind::Failed
                    } else {
                        AlertKind::Finished
                    });
                }
                runtime.set_running(id, false)?;
                if let Some(run) = chat.run.as_mut() {
                    run.settled = true;
                    finish = true;
                }
                if let Some(path) = file_of(index, id, chat.draft.as_ref()) {
                    index.refresh_file(&path)?;
                }
                // The file has the run's messages now.
                if let Some(running) = chat.process.as_mut() {
                    running.live.clear_messages();
                }
                list_changed = true;
                view_changed = true;
            }
            Event::ExtensionUiRequest {
                id: question,
                method,
                title,
                message,
                options,
                prefill,
                timeout,
                ..
            } => {
                let dialog = match method.as_str() {
                    "select" => Dialog::Select,
                    "confirm" => Dialog::Confirm,
                    "input" => Dialog::Input,
                    "editor" => Dialog::Editor,
                    // `notify` and the like expect no answer.
                    _ => return Ok(()),
                };
                if running.questions.is_empty() {
                    alert = Some(AlertKind::Input);
                }
                running.questions.push(Question {
                    id: question.clone(),
                    dialog,
                    title: title.clone().unwrap_or_default(),
                    message: message.clone(),
                    options: options.clone().unwrap_or_default(),
                    prefill: prefill.clone(),
                });
                if let Some(timeout) = timeout {
                    let wait = Duration::from_millis(*timeout);
                    let expire =
                        self.clone()
                            .expire(id.to_owned(), pi.clone(), question.clone(), wait);
                    tokio::spawn(expire);
                }
                list_changed = true;
                view_changed = true;
            }
            Event::SessionInfoChanged { .. } => {
                if let Some(path) = file_of(index, id, chat.draft.as_ref()) {
                    index.refresh_file(&path)?;
                }
                list_changed = true;
                view_changed = true;
            }
            _ => {}
        }
        if view_changed {
            chat.changed();
        }
        let settled = matches!(event, Event::AgentSettled { .. });
        drop(state);
        if let Some(kind) = alert {
            self.alert(id, kind);
        }
        if finish {
            let (engine, id) = (self.clone(), id.to_owned());
            tokio::spawn(async move {
                let Ok(ops) = engine.ops(&id) else { return };
                let _ops = ops.lock().await;
                engine.finish_run(&id).await;
            });
        }
        if settled {
            self.save_index()?;
        }
        if list_changed {
            self.changed();
        }
        Ok(())
    }

    /// Stop `pi` if its chat is still idle, and return whether it did. A
    /// command that's starting a run, or a run that's going, keeps it.
    async fn stop_idle(&self, id: &str, pi: &Arc<Pi>) -> Result<bool> {
        let Some(ops): Option<OwnedMutexGuard<()>> = self.state().chats.get(id).and_then(|chat| {
            let ops = chat.ops.clone();
            ops.try_lock_owned().ok()
        }) else {
            return Ok(false);
        };
        let working = {
            let state = self.state();
            let running = state.chats.get(id).and_then(|chat| chat.process.as_ref());
            // An open dialog keeps pi too: an extension command waits on it.
            running.is_some_and(|r| r.live.working() || !r.questions.is_empty())
        };
        if working {
            return Ok(false);
        }
        pi.stop(self.0.config.grace).await?;
        // Detached while `ops` is held, so the next command starts a new pi.
        self.detach(id, pi)?;
        drop(ops);
        Ok(true)
    }

    /// Clean up after `pi` exited, by itself or because it was stopped.
    fn exited(&self, id: &str, pi: &Arc<Pi>) -> Result<()> {
        self.detach(id, pi)?;
        self.changed();
        Ok(())
    }

    /// Forget `pi` as its chat's process, if it still is, and tell about a
    /// run it cut short.
    fn detach(&self, id: &str, pi: &Arc<Pi>) -> Result<()> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
            stopping,
        } = &mut *state;
        runtime.remove(id, pi.pid())?;
        let Some(chat) = chats.get_mut(id) else {
            return Ok(());
        };
        let Some(running) = chat.process.take_if(|r| Arc::ptr_eq(&r.pi, pi)) else {
            return Ok(());
        };
        // A run that didn't settle has no end to diff against.
        chat.run.take_if(|run| !run.settled);
        if let Some(path) = file_of(index, id, chat.draft.as_ref()) {
            index.refresh_file(&path)?;
        }
        if running.live.working() {
            index.set_interrupted(id);
        }
        // A draft pi never wrote, whose pi stopped idle, has nothing to show
        // or resume. One cut off mid-run stays, so its message can be sent
        // again.
        let unwritten = |draft: &Draft| draft.path.as_deref().and_then(|p| index.get(p)).is_none();
        let dropped = !running.live.working() && chat.draft.take_if(|d| unwritten(d)).is_some();
        // An idle pi leaves nothing a viewer sees but its questions; a
        // working one leaves the chat interrupted.
        if dropped || running.live.working() || !running.questions.is_empty() {
            chat.changed();
        }
        let interrupted = running.live.working() && !*stopping;
        drop(state);
        if interrupted {
            self.alert(id, AlertKind::Interrupted);
        }
        Ok(())
    }

    fn alert(&self, id: &str, kind: AlertKind) {
        let alert = Alert {
            chat: id.to_owned(),
            kind,
        };
        // With nobody listening, there's no one to tell.
        drop(self.0.alerts.send(alert));
    }

    /// Bring the index up to date with what the sessions folder reported,
    /// and tell the list and the open chats what changed.
    fn sync(&self, changes: Changes) {
        let sessions_dir = &self.0.config.sessions_dir;
        let mut state = self.state();
        let prompts: HashMap<PathBuf, Option<i64>> = if changes.rescan {
            let rows = state.index.iter();
            rows.map(|(path, summary)| (path.to_owned(), summary.last_user_at))
                .collect()
        } else {
            let paths = changes.paths.iter();
            let rows = paths.map(|path| (path.clone(), state.index.get(path)));
            rows.map(|(path, summary)| (path, summary.and_then(|s| s.last_user_at)))
                .collect()
        };
        let changed = if changes.rescan {
            state.index.refresh(sessions_dir).unwrap_or_else(|e| {
                eprintln!("rescanning {}: {e:#}", sessions_dir.display());
                Vec::new()
            })
        } else {
            let paths = changes.paths.into_iter();
            let sessions = paths.filter(|path| index::is_session_file(sessions_dir, path));
            let refreshed = sessions.filter(|path| {
                state.index.refresh_file(path).unwrap_or_else(|e| {
                    eprintln!("reading {}: {e:#}", path.display());
                    false
                })
            });
            refreshed.collect()
        };
        if changed.is_empty() {
            return;
        }
        let mut unreported = Vec::new();
        let State { index, chats, .. } = &mut *state;
        for path in &changed {
            let Some(summary) = index.get(path) else {
                continue;
            };
            let prompt = summary.last_user_at;
            let new_prompt = prompt.filter(|at| prompts.get(path) != Some(&Some(*at)));
            match (new_prompt, chats.get_mut(&summary.id)) {
                (Some(at), Some(chat)) if !chat.sent.contains(&at) => {
                    if chat.process.is_some() {
                        unreported.push((summary.id.clone(), at));
                    } else {
                        chat.elsewhere = true;
                    }
                    chat.changed();
                }
                (_, Some(chat)) => chat.changed(),
                (Some(_), None) => {
                    let chat = Chat {
                        elsewhere: true,
                        ..Chat::default()
                    };
                    chats.insert(summary.id.clone(), chat);
                }
                (None, None) => {}
            }
        }
        drop(state);
        self.changed();
        for (id, at) in unreported {
            tokio::spawn(self.clone().unless_reported(id, at));
        }
    }

    /// Wait for the chat's own pi to report the prompt sent at `at`. If it
    /// doesn't, another pi wrote it: the chat's pi has a stale view of the
    /// file now, so it's stopped, and sending waits for a take-over.
    async fn unless_reported(self, id: String, at: i64) {
        tokio::time::sleep(REPORT_WAIT).await;
        let pi = {
            let mut state = self.state();
            let Some(chat) = state
                .chats
                .get_mut(&id)
                .filter(|chat| !chat.sent.contains(&at))
            else {
                return;
            };
            chat.elsewhere = true;
            chat.changed();
            chat.process.as_ref().map(|running| running.pi.clone())
        };
        self.changed();
        let Some(pi) = pi else { return };
        let Ok(ops) = self.ops(&id) else { return };
        let _ops = ops.lock().await;
        if let Err(e) = pi.stop(self.0.config.grace).await {
            eprintln!("chat {id}: stopping pi: {e:#}");
        }
        if let Err(e) = self.detach(&id, &pi) {
            eprintln!("chat {id}: {e:#}");
        }
    }

    /// Drop `pi`'s question when its dialog's own timeout ends it, since pi
    /// doesn't say so.
    async fn expire(self, id: String, pi: Arc<Pi>, question: String, wait: Duration) {
        tokio::time::sleep(wait).await;
        let removed = {
            let mut state = self.state();
            let running = state
                .chats
                .get_mut(&id)
                .and_then(|chat| chat.process.as_mut());
            let running = running.filter(|r| Arc::ptr_eq(&r.pi, &pi));
            running.is_some_and(|r| {
                let before = r.questions.len();
                r.questions.retain(|q| q.id != question);
                r.questions.len() != before
            })
        };
        if removed {
            self.asked(&id);
        }
    }

    /// Tell the chat's and the list's viewers that its questions changed.
    fn asked(&self, id: &str) {
        if let Some(chat) = self.state().chats.get(id) {
            chat.changed();
        }
        self.changed();
    }

    fn save_index(&self) -> Result<()> {
        let path = self.0.config.state_dir.join("index.json");
        self.state().index.save(&path)
    }

    fn changed(&self) {
        self.0.list.send_modify(|revision| *revision += 1);
    }
}

/// Apply the sessions folder's changes to the engine until it's gone. The
/// task holds the engine weakly, so dropping the engine ends it.
async fn follow(engine: Weak<Inner>, watcher: Watcher) {
    loop {
        let changes = watcher.changes().await;
        let Some(inner) = engine.upgrade() else {
            return;
        };
        let engine = Engine(inner);
        if let Err(e) = tokio::task::spawn_blocking(move || engine.sync(changes)).await {
            eprintln!("following the sessions folder: {e:#}");
        }
    }
}

/// What a running pi says about its model and context.
async fn live_composer(pi: &Pi) -> Result<Composer> {
    let (state, stats) = tokio::try_join!(
        pi.call(json!({"type": "get_state"})),
        pi.call(json!({"type": "get_session_stats"})),
    )?;
    let model = Model::from_pi(&state["model"]);
    let usage = &stats["contextUsage"];
    Ok(Composer {
        thinking: state["thinkingLevel"].as_str().map(str::to_owned),
        context_tokens: usage["tokens"].as_u64(),
        context_window: usage["contextWindow"]
            .as_u64()
            .or(model.as_ref().and_then(|model| model.context_window)),
        model: model.map(|model| model.id),
        spent: Spent {
            tokens: stats["tokens"]["total"].as_u64().unwrap_or_default(),
            cost: stats["cost"].as_f64().unwrap_or_default(),
        },
    })
}

/// Fail unless `id` is a chat in the index or a draft.
fn known(state: &State, id: &str) -> Result<()> {
    if !state.chats.contains_key(id) && state.index.find(id).is_none() {
        bail!("no chat {id}");
    }
    Ok(())
}

/// The chat's folder: the session's, or the draft's.
fn cwd_of(state: &State, id: &str) -> Option<PathBuf> {
    let draft = state.chats.get(id).and_then(|chat| chat.draft.as_ref());
    let session = state.index.find(id).map(|(_, s)| s.cwd.clone());
    session.or_else(|| draft.map(|draft| draft.cwd.clone()))
}

/// The chat's session file: the index's, or the one pi named for a draft.
fn file_of(index: &Index, id: &str, draft: Option<&Draft>) -> Option<PathBuf> {
    index
        .find(id)
        .map(|(path, _)| path.to_owned())
        .or_else(|| draft?.path.clone())
}

/// A chat in the index, as the list shows it.
fn info(state: &State, summary: &Summary) -> ChatInfo {
    let chat = state.chats.get(&summary.id);
    ChatInfo {
        id: summary.id.clone(),
        cwd: summary.cwd.clone(),
        created: summary.created,
        status: status(chat, Some(summary), state.index.is_interrupted(&summary.id)),
        title: chat
            .and_then(|chat| chat.named.clone())
            .or_else(|| summary.title.clone()),
        summary: Some(summary.clone()),
        elsewhere: chat.is_some_and(|chat| chat.elsewhere),
    }
}

/// A draft pi hasn't written yet, as the list shows it.
fn draft_info(state: &State, id: &str, chat: &Chat) -> Option<ChatInfo> {
    let draft = chat.draft.as_ref()?;
    let written = draft.path.as_deref().and_then(|path| state.index.get(path));
    written.is_none().then(|| ChatInfo {
        id: id.to_owned(),
        cwd: draft.cwd.clone(),
        created: draft.created,
        summary: None,
        title: chat.named.clone(),
        status: status(Some(chat), None, false),
        elsewhere: chat.elsewhere,
    })
}

/// What a chat is doing, from its process if it has one, else from whether
/// the engine cut its run short, else from how its last run ended in the
/// file.
fn status(chat: Option<&Chat>, summary: Option<&Summary>, interrupted: bool) -> Status {
    if let Some(running) = chat.and_then(|chat| chat.process.as_ref()) {
        let live = &running.live;
        if !running.questions.is_empty() {
            return Status::AwaitingInput;
        }
        if live.working() {
            return Status::Working;
        }
        if live.failure().is_some() {
            return Status::Failed;
        }
    }
    if interrupted {
        return Status::Interrupted;
    }
    match summary.and_then(|summary| summary.outcome) {
        Some(Outcome::Failed) => Status::Failed,
        _ => Status::Idle,
    }
}
