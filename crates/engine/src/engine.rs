mod jobs;
mod queue;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::{OwnedMutexGuard, broadcast, mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;

use crate::agent_tools;
use crate::attachments::{self, Attachment};
use crate::index::{self, Index};
use crate::live::{self, Live};
use crate::login;
use crate::models::{Available, Catalog, Model, Sources};
use crate::pi::{Event, Options, Pi, Session};
use crate::pi_setup;
use crate::quota;
pub use crate::relay::SecretStore;
use crate::relay::{self, DEFAULT_RELAY, Link, LinkStatus, Pairing, Phone, Phones};
use crate::runtime::Runtime;
use crate::seal;
use crate::session::{Outcome, Spent, Summary};
use crate::settings::{SendBehavior, Settings};
use crate::sign_in::{self, SignIn};

use crate::skills::{self, Skill};
use crate::title::Titler;
use crate::transcript::{self, Image, Page, Tail, ToolOutput, Transcripts, Window};
use crate::turns::{Counts, Diff, Snapshot, Turns};
use crate::watch::{Changes, Watcher};
pub(crate) use jobs::Request as JobRequest;
pub use queue::Queued;

const WATCH_LATENCY: Duration = Duration::from_millis(100);

/// Allow file writes and pi's prompt events to arrive in either order.
const REPORT_WAIT: Duration = Duration::from_secs(1);

const SENT_KEPT: usize = 64;
const ALERTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub pi: PathBuf,
    pub sessions_dir: PathBuf,
    pub state_dir: PathBuf,
    pub idle: Duration,
    pub grace: Duration,
    pub home: PathBuf,
    /// `None` outside Ludolph.app; development engines must not change its login item.
    pub app: Option<PathBuf>,
    /// Ludolph's bundled pi extensions.
    pub extensions: PathBuf,
    pub secrets: SecretStore,
}

impl Config {
    pub fn attachments_dir(&self) -> PathBuf {
        self.sessions_dir.with_file_name("attachments")
    }

    pub fn scratch_dir(&self) -> PathBuf {
        self.state_dir.join("scratch")
    }

    pub fn is_project(&self, cwd: &Path) -> bool {
        cwd != self.home && !cwd.starts_with(self.scratch_dir())
    }
}

#[derive(Debug, Clone)]
pub struct Engine(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    config: Config,
    state: Mutex<State>,
    list: watch::Sender<u64>,
    alerts: broadcast::Sender<Alert>,
    jobs: jobs::Jobs,
    transcripts: Arc<Mutex<Transcripts>>,
    watching: AbortHandle,
    titler: Arc<Titler>,
    /// Hold through persistence so concurrent settings writes land in order.
    settings: Mutex<Settings>,
    turns: Arc<Turns>,
    catalog: Catalog,
    relay: Mutex<Option<AbortHandle>>,
    phones: Arc<Phones>,
    phone_links: Mutex<()>,
    phone: watch::Sender<Option<Phone>>,
    pairing_task: Mutex<Option<AbortHandle>>,
    pairing: watch::Sender<Option<Pairing>>,
    /// The latest attachment cleanup; each waits for the one before, so they never race.
    cleanup: Mutex<Option<JoinHandle<()>>>,
    /// Each resource change rewrites pi's settings, so they run one at a time.
    resources: tokio::sync::Mutex<()>,
    sign_in: SignIn,
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
    /// The latest subscription quota by provider, for every chat on it.
    quotas: HashMap<String, Vec<quota::Window>>,
    stopping: bool,
}

#[derive(Debug)]
struct Chat {
    /// Held by commands that start or stop the chat's pi, so they don't overlap.
    ops: Arc<tokio::sync::Mutex<()>>,
    draft: Option<Draft>,
    process: Option<Running>,
    /// Stop generation: reject sends that began before the latest Stop.
    stops: u64,
    sent: VecDeque<i64>,
    waiting: VecDeque<queue::Waiting>,
    job_results: VecDeque<String>,
    /// Another pi wrote a prompt to the session, so sending waits for a
    /// take-over.
    elsewhere: bool,
    /// Newer than the session file until pi records the rename.
    named: Option<String>,
    titling: bool,
    run: Option<Run>,
    latest_turn: Option<i64>,
    turns_revision: u64,
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
            waiting: VecDeque::new(),
            job_results: VecDeque::new(),
            elsewhere: false,
            named: None,
            titling: false,
            run: None,
            latest_turn: None,
            turns_revision: 0,
            changes: watch::channel(0).0,
        }
    }
}

impl Chat {
    fn changed(&self) {
        self.changes.send_modify(|revision| *revision += 1);
    }

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

#[derive(Debug)]
struct Run {
    before: Snapshot,
    /// User-message timestamp; views use it to identify this run.
    key: Option<i64>,
    settled: bool,
}

#[derive(Debug, Clone)]
struct Draft {
    cwd: PathBuf,
    path: Option<PathBuf>,
    created: DateTime<Utc>,
}

#[derive(Debug)]
struct Running {
    pi: Arc<Pi>,
    sources: Sources,
    live: Live,
    questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub dialog: Dialog,
    pub title: String,
    pub message: Option<String>,
    pub options: Vec<String>,
    pub prefill: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Select,
    Confirm,
    Input,
    Editor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Value(String),
    Confirmed(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatInfo {
    pub id: String,
    pub cwd: PathBuf,
    pub created: DateTime<Utc>,
    pub summary: Option<Summary>,
    pub title: Option<String>,
    pub status: Status,
    pub elsewhere: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub chat: ChatInfo,
    pub page: Page,
    pub revision: u64,
    pub latest_turn: Option<i64>,
    /// Changes on run boundaries and saved diffs, not streamed text.
    pub turns_revision: u64,
    pub question: Option<Question>,
    pub run: Option<live::Run>,
    pub failure: Option<String>,
    pub queued: Vec<Queued>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working,
    AwaitingInput,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub chat: String,
    pub kind: AlertKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertKind {
    Finished,
    /// A failed run; explicitly stopped runs don't alert.
    Failed,
    Interrupted,
    Input,
    Job {
        id: String,
        title: String,
        status: String,
    },
}

#[derive(Debug)]
enum Prompt<'a> {
    User(&'a str, Vec<Attachment>),
    Job(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Started,
    Queued,
    /// An extension consumed the input without starting a run.
    Handled,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Composer {
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub spent: Spent,
}

impl Engine {
    pub async fn start(config: Config) -> Result<Self> {
        std::fs::create_dir_all(&config.state_dir)
            .with_context(|| format!("creating {}", config.state_dir.display()))?;
        let jobs = jobs::Jobs::open(&config.state_dir)?;
        let runtime_path = config.state_dir.join("runtime.json");
        let (runtime, interrupted) = Runtime::reap(&runtime_path, config.grace).await?;
        std::fs::create_dir_all(&config.sessions_dir)
            .with_context(|| format!("creating {}", config.sessions_dir.display()))?;
        // Watching starts first, so writes during the refresh aren't missed.
        let agent = config
            .sessions_dir
            .parent()
            .context("pi's sessions folder has no parent")?;
        let watcher = Watcher::start(agent, WATCH_LATENCY)?;
        let index_path = config.state_dir.join("index.json");
        let sessions_dir = config.sessions_dir.clone();
        let index = tokio::task::spawn_blocking(move || -> Result<Index> {
            // The index is disposable; session files remain the source of truth.
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
            quotas: HashMap::new(),
            stopping: false,
        };
        let settings = Settings::load(&config.state_dir)?;
        if let Err(e) = login::apply(&config.home, config.app.as_deref(), settings.open_at_login) {
            eprintln!("login item: {e:#}");
        }
        // `turns` held patches, which `changes` replaced.
        match std::fs::remove_dir_all(config.state_dir.join("turns")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                eprintln!("removing the old turn changes: {e}");
            }
            _ => {}
        }
        let turns = Arc::new(Turns::new(config.state_dir.join("changes"))?);
        // The engine's folder has no `.pi` settings to change the list.
        let catalog = Catalog::new(
            config.pi.clone(),
            config.state_dir.clone(),
            agent,
            config.grace,
        )?;
        let phones = Arc::new(Phones::new(config.state_dir.join("relay-replays"))?);
        // Following starts once the engine exists: inotify reports the writes made while
        // starting, and a weak reference made before the engine can't reach it.
        let (start, started) = oneshot::channel();
        let watching = tokio::spawn(async move {
            if let Ok(engine) = started.await {
                follow(engine, watcher).await;
            }
        });
        let engine = Self(Arc::new(Inner {
            turns,
            catalog,
            state: Mutex::new(state),
            list: watch::channel(0).0,
            alerts: broadcast::channel(ALERTS).0,
            jobs,
            transcripts: Arc::default(),
            watching: watching.abort_handle(),
            titler: Arc::new(Titler::new(config.pi.clone())),
            settings: Mutex::new(settings),
            relay: Mutex::default(),
            phones,
            phone_links: Mutex::default(),
            phone: watch::channel(None).0,
            pairing_task: Mutex::default(),
            pairing: watch::channel(None).0,
            cleanup: Mutex::default(),
            resources: tokio::sync::Mutex::default(),
            sign_in: SignIn::default(),
            config,
        }));
        drop(start.send(Arc::downgrade(&engine.0)));
        tokio::spawn(engine.clone().reconnect());
        Ok(engine)
    }

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

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.list.subscribe()
    }

    pub fn alerts(&self) -> broadcast::Receiver<Alert> {
        self.0.alerts.subscribe()
    }

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

    pub async fn view(&self, id: &str, window: Window) -> Result<View> {
        let (
            chat,
            path,
            messages,
            streaming,
            revision,
            question,
            run,
            failure,
            queued,
            latest_turn,
            turns_revision,
            calls,
        ) = {
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
            let failure = live.and_then(Live::failure).map(str::to_owned);
            let revision = chat.map_or(0, |chat| *chat.changes.borrow());
            let queued = Self::queued(chat);
            (
                info,
                path,
                messages,
                streaming,
                revision,
                question,
                run,
                failure,
                queued,
                chat.and_then(|c| c.latest_turn),
                chat.map_or(0, |c| c.turns_revision),
                live.map(|live| live.calls().clone()).unwrap_or_default(),
            )
        };
        let transcripts = self.0.transcripts.clone();
        let page = tokio::task::spawn_blocking(move || {
            let tail = Tail {
                messages: &messages,
                streaming: streaming.as_ref(),
                calls: Some(&calls),
            };
            let mut transcripts = transcripts.lock().expect("lock poisoned");
            transcripts.page(path.as_deref().filter(|p| p.exists()), tail, &window)
        })
        .await??;
        Ok(View {
            chat,
            page,
            revision,
            latest_turn,
            turns_revision,
            question,
            run,
            failure,
            queued,
        })
    }

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

    pub fn config(&self) -> &Config {
        &self.0.config
    }

    /// Keep a new chat as a draft until pi writes its first message.
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

    pub async fn send(
        &self,
        id: &str,
        text: &str,
        images: Vec<Attachment>,
    ) -> Result<(Delivery, String)> {
        let ops = self.ops(id)?;
        let stops = self.stops(id);
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let text = attachments::resolve(&self.0.config.attachments_dir(), id, text, &images)?;
        let hold = self.settings().send_behavior == SendBehavior::Queue && {
            let state = self.state();
            state.chats.get(id).is_some_and(|chat| {
                !chat.waiting.is_empty() || chat.process.as_ref().is_some_and(|r| r.live.working())
            })
        };
        if hold {
            self.enqueue(id, &text, images, stops)?;
            return Ok((Delivery::Queued, text));
        }
        let delivery = self
            .send_locked(id, Prompt::User(&text, images), stops)
            .await?;
        Ok((delivery, text))
    }

    /// Send while holding the chat's command lock, bypassing the waiting queue.
    async fn send_locked(&self, id: &str, input: Prompt<'_>, stops: u64) -> Result<Delivery> {
        let pi = self.current_process(id).await?;
        self.finish_run(id).await;
        let (working, cwd) = {
            let state = self.state();
            let chat = state.chats.get(id).expect("a chat with a process is known");
            let working = chat.process.as_ref().is_some_and(|r| r.live.working());
            (working, cwd_of(&state, id))
        };
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
        let title_from = match &input {
            Prompt::User(text, _) => Some(*text),
            Prompt::Job(_) => None,
        };
        let delivery = self.prompt(id, &pi, input, stops).await;
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
        if delivery != Delivery::Handled
            && let Some(text) = title_from
        {
            self.title_soon(id, text);
        }
        Ok(delivery)
    }

    fn live_of<T>(&self, id: &str, pi: &Arc<Pi>, f: impl FnOnce(&mut Live) -> T) -> Option<T> {
        let mut state = self.state();
        let running = state.chats.get_mut(id)?.process.as_mut()?;
        Arc::ptr_eq(&running.pi, pi).then(|| f(&mut running.live))
    }

    async fn prompt(&self, id: &str, pi: &Pi, input: Prompt<'_>, stops: u64) -> Result<Delivery> {
        let (text, images) = match input {
            Prompt::User(text, images) => (text, images),
            Prompt::Job(message) => {
                let text = format!("/{} {}", jobs::CUSTOM_TYPE, serde_json::to_string(message)?);
                let answer = pi
                    .call_checked(json!({"type": "prompt", "message": text}), || {
                        self.check_stops(id, stops)
                    })
                    .await?;
                if answer["disposition"] != "handled" {
                    bail!("pi did not handle the background job continuation");
                }
                return Ok(Delivery::Started);
            }
        };
        // pi ignores `steer` when no run is going, so the answer says which
        // happened, even if a run started or ended meanwhile.
        let expanded = if ludolph_prompt::skill_words(text)
            .iter()
            .any(|(_, skill)| *skill)
        {
            let commands = pi.call(json!({"type": "get_commands"})).await?;
            let skills = skills::from_commands(&commands);
            let text = text.to_owned();
            Some(tokio::task::spawn_blocking(move || skills::expand(&text, &skills)).await??)
        } else {
            None
        };
        let text = expanded.as_deref().unwrap_or(text);
        let mut command = json!({"type": "prompt", "message": text, "streamingBehavior": "steer"});
        if !images.is_empty() {
            let root = self.0.config.attachments_dir();
            let session = id.to_owned();
            let images =
                tokio::task::spawn_blocking(move || attachments::save(&root, &session, images))
                    .await??;
            let images = images.into_iter().map(
                |image| json!({"type": "image", "mimeType": image.mime_type, "data": image.data}),
            );
            command["images"] = Value::Array(images.collect());
        }
        let answer = pi
            .call_checked(command, || self.check_stops(id, stops))
            .await?;
        match answer["disposition"].as_str() {
            Some("started") => Ok(Delivery::Started),
            Some("queued") => Ok(Delivery::Queued),
            Some("handled") => Ok(Delivery::Handled),
            other => bail!("pi answered the prompt with {other:?}"),
        }
    }

    fn check_stops(&self, id: &str, stops: u64) -> Result<()> {
        if self.stops(id) != stops {
            bail!("stopped before it was sent");
        }
        Ok(())
    }

    /// Callers hold `ops` so the next run cannot start before this snapshot.
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
            let changes = self.0.turns.after(id, before).await?;
            let (turns, session) = (self.0.turns.clone(), id.to_owned());
            tokio::task::spawn_blocking(move || turns.save(&session, key, &changes)).await?
        };
        match saved.await {
            Ok(()) => {
                if let Some(chat) = self.state().chats.get_mut(id) {
                    chat.turns_revision += 1;
                    chat.changed();
                }
            }
            Err(e) => eprintln!("chat {id}: saving this run's changes: {e:#}"),
        }
    }

    /// The session's details, its spend, and its provider's subscription quota.
    pub async fn details(
        &self,
        id: &str,
    ) -> Result<(transcript::Details, Spent, Vec<quota::Window>)> {
        let (path, spent, quota) = {
            let state = self.state();
            known(&state, id)?;
            let summary = state.index.find(id);
            let info = match summary {
                Some((_, summary)) => Some(info(&state, summary)),
                None => state
                    .chats
                    .get(id)
                    .and_then(|chat| draft_info(&state, id, chat)),
            };
            (
                file_of(
                    &state.index,
                    id,
                    state.chats.get(id).and_then(|chat| chat.draft.as_ref()),
                ),
                summary.map_or_else(Spent::default, |(_, summary)| summary.spent),
                info.map_or_else(Vec::new, |info| quota_of(&state, &info)),
            )
        };
        let transcripts = self.0.transcripts.clone();
        let details =
            tokio::task::spawn_blocking(move || match path.filter(|path| path.exists()) {
                Some(path) => transcripts.lock().expect("lock poisoned").details(&path),
                None => Ok(transcript::Details::default()),
            })
            .await??;
        Ok((details, spent, quota))
    }

    pub async fn chat_changes(&self, id: &str) -> Result<Diff> {
        known(&self.state(), id)?;
        self.0.turns.chat(id).await
    }

    pub async fn chat_counts(&self, id: &str) -> Result<Counts> {
        known(&self.state(), id)?;
        self.0.turns.chat_counts(id).await
    }

    pub async fn run_counts(&self, id: &str) -> Result<BTreeMap<i64, Counts>> {
        let (turns, id) = (self.0.turns.clone(), id.to_owned());
        tokio::task::spawn_blocking(move || turns.counts(&id)).await?
    }

    pub async fn run_changes(&self, id: &str, key: i64) -> Result<Option<Diff>> {
        self.0.turns.load(id, key).await
    }

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

    pub async fn move_chat(&self, id: &str, cwd: &Path) -> Result<()> {
        if !cwd.is_absolute() {
            bail!("the project path must be absolute");
        }
        if !cwd.is_dir() {
            bail!("{} isn't a folder", cwd.display());
        }
        let ops = self.ops(id)?;
        let _ops = ops
            .try_lock()
            .context("this chat is busy; try again once it is idle")?;
        self.refuse_elsewhere(id)?;
        let pi = {
            let state = self.state();
            let chat = state.chats.get(id).context("no such chat")?;
            if !chat.waiting.is_empty()
                || chat.run.is_some()
                || chat
                    .process
                    .as_ref()
                    .is_some_and(|r| r.live.working() || !r.questions.is_empty())
            {
                bail!("stop the agent and clear queued messages before moving this chat");
            }
            let (_, summary) = state
                .index
                .find(id)
                .context("send a message before moving this chat")?;
            if summary.cwd == cwd {
                return Ok(());
            }
            chat.process.as_ref().map(|r| r.pi.clone())
        };
        if let Some(pi) = pi {
            pi.stop(self.0.config.grace).await?;
            self.detach(id, &pi)?;
        }
        let (source, copies) = {
            let state = self.state();
            let (source, _) = state.index.find(id).context("the session is gone")?;
            let copies: Vec<_> = state
                .index
                .iter()
                .filter(|(_, summary)| summary.id == id)
                .map(|(path, _)| path.to_owned())
                .collect();
            (source.to_owned(), copies)
        };
        // Copying a large session takes a while; other chats keep the state lock meanwhile.
        let (sessions, dest) = (self.0.config.sessions_dir.clone(), cwd.to_owned());
        let (copy, target) = tokio::task::spawn_blocking(move || {
            crate::session::relocate(&source, &sessions, &dest)
        })
        .await??;
        {
            let mut state = self.state();
            // Published under the lock, so the watcher can't take the moved prompt for another pi's.
            copy.persist_noclobber(&target)
                .with_context(|| format!("moving the session to {}", target.display()))?;
            state.index.refresh_file(&target)?;
            for path in copies {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
                state.index.refresh_file(&path)?;
            }
            if let Some(chat) = state.chats.get_mut(id) {
                chat.draft = None;
                chat.changed();
            }
        }
        self.save_index()?;
        self.changed();
        Ok(())
    }

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
        self.0.jobs.cancel_chat(id);
        self.0.turns.forget(id)?;
        self.save_index()?;
        self.changed();
        // Folders of live sessions and drafts are referenced or about to be; only orphans need a scan.
        let engine = self.clone();
        let protected = move || {
            let state = engine.state();
            let sessions = state.index.chats().map(|(_, summary)| summary.id.clone());
            sessions.chain(state.chats.keys().cloned()).collect()
        };
        let (root, sessions, id) = (
            self.0.config.attachments_dir(),
            self.0.config.sessions_dir.clone(),
            id.to_owned(),
        );
        // Scanning every session file can take seconds, so the delete doesn't wait for it.
        let mut cleanup = self
            .0
            .cleanup
            .lock()
            .expect("the cleanup lock isn't poisoned");
        let previous = cleanup.take();
        *cleanup = Some(tokio::spawn(async move {
            if let Some(previous) = previous {
                _ = previous.await;
            }
            let removed = tokio::task::spawn_blocking(move || {
                attachments::cleanup(&root, &sessions, protected)
            });
            match removed.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("chat {id}: attachment cleanup: {error:#}"),
                Err(error) => eprintln!("chat {id}: attachment cleanup: {error}"),
            }
        }));
        Ok(())
    }

    /// Restart pi from the changed session file before allowing sends again.
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

    pub async fn polish(&self, heard: &str) -> Result<String> {
        let model = self.settings().sidecar_model;
        self.0.titler.polish(heard, model.as_deref()).await
    }

    pub fn settings(&self) -> Settings {
        self.0
            .settings
            .lock()
            .expect("the settings lock isn't poisoned")
            .clone()
    }

    /// The user's skills and extensions, after `change` if given. Chats pick changes up when their
    /// pi next starts.
    pub async fn resources(&self, change: Option<&pi_setup::Change>) -> Result<Value> {
        let _one = self.0.resources.lock().await;
        self.pi_setup(json!({"topic": "resources", "change": change}))
            .await
    }

    /// The providers pi can sign in to, and where it finds credentials for each.
    pub async fn accounts(&self) -> Result<Value> {
        self.pi_setup(json!({"topic": "accounts"})).await
    }

    async fn pi_setup(&self, request: Value) -> Result<Value> {
        let config = &self.0.config;
        pi_setup::ask(&config.pi, &config.extensions, config.grace, request).await
    }

    pub fn sign_in(&self) -> &SignIn {
        &self.0.sign_in
    }

    pub fn start_sign_in(&self, provider: &str, api_key: bool) -> Result<sign_in::State> {
        let config = &self.0.config;
        self.0
            .sign_in
            .start(&config.pi, &config.extensions, provider, api_key)
    }

    /// Removes the credentials pi stored for `provider`. Environment variables and models.json stay.
    pub async fn sign_out(&self, provider: &str) -> Result<Value> {
        self.pi_setup(json!({"topic": "signOut", "provider": provider}))
            .await?;
        self.accounts().await
    }

    pub async fn set_settings(&self, settings: Settings) -> Result<Settings> {
        for model in [&settings.sidecar_model, &settings.gui_model]
            .into_iter()
            .flatten()
        {
            if self.0.catalog.find(model).await?.is_none() {
                bail!("pi has no model {model}");
            }
        }
        let config = &self.0.config;
        login::apply(&config.home, config.app.as_deref(), settings.open_at_login)?;
        let mut saved = self
            .0
            .settings
            .lock()
            .expect("the settings lock isn't poisoned");
        settings.save(&self.0.config.state_dir)?;
        let picked = saved.chat_models != settings.chat_models;
        *saved = settings.clone();
        drop(saved);
        if picked {
            self.0.catalog.changed();
        }
        Ok(settings)
    }

    pub fn phone(&self) -> Option<Phone> {
        self.0.phone.borrow().clone()
    }

    pub fn linked_phones(&self) -> Vec<String> {
        self.0.phones.ids()
    }

    pub fn pairing(&self) -> Option<Pairing> {
        self.0.pairing.borrow().clone()
    }

    pub fn relay(&self) -> String {
        self.phone()
            .map_or_else(|| DEFAULT_RELAY.to_owned(), |phone| phone.relay)
    }

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
            Err(e) if token.is_some() && relay::is_rejected(&e) => {
                relay::pair(relay, None, &secret, shown).await?
            }
            other => other?,
        };
        let config = &self.0.config;
        // Pair completion and per-phone unlink both rewrite the same Keychain item.
        let _links = self
            .0
            .phone_links
            .lock()
            .expect("the phone links lock isn't poisoned");
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

    pub fn unlink_phone(&self, phone: &str) -> Result<()> {
        let _links = self
            .0
            .phone_links
            .lock()
            .expect("the phone links lock isn't poisoned");
        let config = &self.0.config;
        let mut link = Link::load(&config.state_dir, config.secrets)?
            .context("this Mac has no linked phones")?;
        let mut found = None;
        for (i, secret) in link.phones.iter().enumerate() {
            if seal::Keys::derive(secret)?.phone == phone {
                found = Some(i);
                break;
            }
        }
        let at = found.context("this phone isn't linked to this Mac")?;
        link.phones.remove(at);
        if !link.phones.is_empty() {
            link.save(&config.state_dir, config.secrets)?;
            self.0.phones.remove(phone);
            return Ok(());
        }
        Link::remove(&config.state_dir, config.secrets)?;
        self.0.phones.remove(phone);
        self.stop_pairing();
        self.disconnect();
        // The phone shows a Mac the relay keeps as offline until it's removed there.
        tokio::spawn(async move {
            if let Err(e) = relay::forget(&link).await {
                eprintln!("relay: the relay may still list this Mac: {e:#}");
            }
        });
        Ok(())
    }

    /// Keychain access may prompt or fail, so it must not delay serving the app.
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

    pub async fn models(&self) -> Result<Available> {
        self.0.catalog.available().await
    }

    pub fn subscribe_models(&self) -> watch::Receiver<u64> {
        self.0.catalog.subscribe()
    }

    pub async fn set_model(
        &self,
        id: &str,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<Composer> {
        let ops = self.ops(id)?;
        self.refuse_elsewhere(id)?;
        let compacting = self.state().chats.get(id).and_then(|chat| {
            chat.process
                .as_ref()
                .filter(|running| running.live.compacting() && running.questions.is_empty())
                .map(|running| running.pi.clone())
        });
        if let Some(pi) = compacting {
            // Prompt preflight may hold ops until compaction finishes, so stop outside it.
            pi.stop(self.0.config.grace).await?;
            self.detach(id, &pi)?;
        }
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let pi = self.current_process(id).await?;
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

    /// Read composer state without starting pi for an idle chat.
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
                Err(e) => eprintln!("chat {id}: asking pi for its skills: {e:#}"),
            }
        }
        let cwd = cwd.with_context(|| format!("no chat {id}"))?;
        self.skills_in(Some(&cwd)).await
    }

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

    pub fn question(&self, id: &str) -> Option<Question> {
        let state = self.state();
        let running = state.chats.get(id)?.process.as_ref()?;
        running.questions.first().cloned()
    }

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

    pub async fn interrupt(&self, id: &str) -> Result<()> {
        let (pi, questions) = {
            let mut state = self.state();
            let chat = state.chats.get_mut(id);
            let running = chat.and_then(|chat| {
                chat.stops += 1;
                chat.waiting.clear();
                chat.job_results.clear();
                chat.changed();
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

    pub async fn shutdown(&self) -> Result<()> {
        self.state().stopping = true;
        self.0.watching.abort();
        self.stop_pairing();
        self.disconnect();
        self.0.jobs.shutdown().await;
        let cleanup = self
            .0
            .cleanup
            .lock()
            .expect("the cleanup lock isn't poisoned")
            .take();
        if let Some(cleanup) = cleanup {
            _ = cleanup.await;
        }
        let (processes, working): (Vec<Arc<Pi>>, Vec<String>) = {
            let state = self.state();
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

    fn ops(&self, id: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let mut state = self.state();
        known(&state, id)?;
        Ok(state.chats.entry(id.to_owned()).or_default().ops.clone())
    }

    fn stops(&self, id: &str) -> u64 {
        self.state().chats.get(id).map_or(0, |chat| chat.stops)
    }

    /// Callers hold `ops`; never replace a process during a run or dialog.
    async fn current_process(&self, id: &str) -> Result<Arc<Pi>> {
        let sources = self.0.catalog.sources()?;
        let stale = {
            let state = self.state();
            match state.chats.get(id).and_then(|chat| chat.process.as_ref()) {
                Some(running)
                    if running.sources != sources
                        && !running.live.working()
                        && running.questions.is_empty() =>
                {
                    Some(running.pi.clone())
                }
                _ => None,
            }
        };
        if let Some(pi) = stale {
            pi.stop(self.0.config.grace).await?;
            self.detach(id, &pi)?;
        }
        self.process(id)
    }

    /// Callers hold the chat's `ops` lock.
    fn process(&self, id: &str) -> Result<Arc<Pi>> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
            stopping,
            ..
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
        let (extensions, mut env) =
            agent_tools::pi_options(&self.0.config.extensions, &self.settings());
        env.push((
            "LUDOLPH_ENGINE_SOCKET".to_owned(),
            self.0
                .config
                .state_dir
                .join("engine.sock")
                .to_string_lossy()
                .into_owned(),
        ));
        let options = Options {
            program: self.0.config.pi.clone(),
            cwd,
            session,
            extensions,
            env,
        };
        let sources = self.0.catalog.sources()?;
        let (pi, events) = Pi::start(&options)?;
        let pi = Arc::new(pi);
        runtime.add(id, pi.pid())?;
        chat.process = Some(Running {
            pi: pi.clone(),
            sources,
            live: Live::new(),
            questions: Vec::new(),
        });
        tokio::spawn(self.clone().watch(id.to_owned(), pi.clone(), events));
        Ok(pi)
    }

    async fn watch(self, id: String, pi: Arc<Pi>, mut events: mpsc::Receiver<Event>) {
        let idle = self.0.config.idle;
        let mut deadline = Some(Instant::now() + idle);
        loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(event) = event else { break };
                    if let Err(e) = self.apply(&id, &pi, &event) {
                        eprintln!("chat {id}: {e:#}");
                    }
                    if matches!(event, Event::AgentStart | Event::AgentSettled { .. }
                        | Event::CompactionStart { .. } | Event::CompactionEnd { .. })
                    {
                        deadline = (self.live_of(&id, &pi, |live| live.working()) != Some(true))
                            .then(|| Instant::now() + idle);
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

    fn set_quota(&self, text: Option<&str>) {
        let Some(text) = text else { return };
        let report: quota::Report = match serde_json::from_str(text) {
            Ok(report) => report,
            Err(e) => {
                eprintln!("reading a quota report: {e:#}");
                return;
            }
        };
        self.state().quotas.insert(report.provider, report.windows);
    }

    fn apply(&self, id: &str, pi: &Arc<Pi>, event: &Event) -> Result<()> {
        if let Event::ExtensionUiRequest {
            method,
            status_key,
            status_text,
            ..
        } = event
            && method == "setStatus"
        {
            if status_key.as_deref() == Some(quota::STATUS_KEY) {
                self.set_quota(status_text.as_deref());
            }
            return Ok(());
        }
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
                && (message["role"] == "user"
                    || (message["role"] == "custom" && message["customType"] == jobs::CUSTOM_TYPE))
            {
                run.key = message["timestamp"].as_i64();
                chat.latest_turn = run.key;
                chat.turns_revision += 1;
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
            Event::CompactionStart { .. } | Event::CompactionEnd { .. } => {
                runtime.set_running(id, running.live.working())?;
            }
            Event::AgentSettled { aborted } => {
                if !aborted {
                    let failed = running.live.failure().is_some();
                    if failed || (chat.waiting.is_empty() && chat.job_results.is_empty()) {
                        alert = Some(if failed {
                            AlertKind::Failed
                        } else {
                            AlertKind::Finished
                        });
                    }
                }
                runtime.set_running(id, false)?;
                if let Some(run) = chat.run.as_mut() {
                    run.settled = true;
                    finish = true;
                }
                if let Some(path) = file_of(index, id, chat.draft.as_ref()) {
                    index.refresh_file(&path)?;
                }
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
            if matches!(event, Event::AgentSettled { aborted: false })
                && self.live_of(id, pi, |live| live.failure().is_none()) == Some(true)
            {
                self.queue_soon(id);
            }
        }
        if list_changed {
            self.changed();
        }
        Ok(())
    }

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

    fn exited(&self, id: &str, pi: &Arc<Pi>) -> Result<()> {
        self.detach(id, pi)?;
        self.changed();
        Ok(())
    }

    fn detach(&self, id: &str, pi: &Arc<Pi>) -> Result<()> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
            stopping,
            ..
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
        // Keep interrupted drafts for retry; empty idle drafts have no history to resume.
        let unwritten = |draft: &Draft| draft.path.as_deref().and_then(|p| index.get(p)).is_none();
        let dropped = !running.live.working() && chat.draft.take_if(|d| unwritten(d)).is_some();
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
        drop(self.0.alerts.send(alert));
    }

    fn sync(&self, changes: Changes) {
        let sessions_dir = &self.0.config.sessions_dir;
        let mut state = self.state();
        let previous: HashMap<PathBuf, (String, Option<i64>)> = if changes.rescan {
            state
                .index
                .iter()
                .map(|(path, summary)| {
                    (path.to_owned(), (summary.id.clone(), summary.last_user_at))
                })
                .collect()
        } else {
            changes
                .paths
                .iter()
                .filter_map(|path| {
                    let summary = state.index.get(path)?;
                    Some((path.clone(), (summary.id.clone(), summary.last_user_at)))
                })
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
            let summary = index.get(path);
            if let Some((id, _)) = previous.get(path)
                && summary.is_none_or(|summary| &summary.id != id)
                && let Some(chat) = chats.get(id)
            {
                chat.changed();
            }
            let Some(summary) = summary else {
                continue;
            };
            let prompt = summary.last_user_at;
            let new_prompt = prompt
                .filter(|at| previous.get(path).and_then(|(_, previous)| *previous) != Some(*at));
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

    /// Stop a stale session writer if pi never acknowledges the prompt found in its file.
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

    /// pi does not report when an extension dialog times out.
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

async fn follow(engine: Weak<Inner>, watcher: Watcher) {
    loop {
        let changes = watcher.changes().await;
        let Some(inner) = engine.upgrade() else {
            return;
        };
        let engine = Engine(inner);
        if (changes.rescan
            || changes
                .paths
                .iter()
                .any(|path| engine.0.catalog.watches(path)))
            && let Err(error) = engine.0.catalog.sources()
        {
            eprintln!("following pi's model settings: {error:#}");
        }
        if let Err(e) = tokio::task::spawn_blocking(move || engine.sync(changes)).await {
            eprintln!("following the sessions folder: {e:#}");
        }
    }
}

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

fn quota_of(state: &State, chat: &ChatInfo) -> Vec<quota::Window> {
    let provider = chat
        .summary
        .as_ref()
        .and_then(|summary| summary.model.as_deref())
        .and_then(|model| model.split_once('/'))
        .map(|(provider, _)| provider);
    provider
        .and_then(|provider| state.quotas.get(provider))
        .map_or_else(Vec::new, |windows| {
            quota::current(windows, Utc::now().timestamp_millis())
        })
}

fn known(state: &State, id: &str) -> Result<()> {
    if !state.chats.contains_key(id) && state.index.find(id).is_none() {
        bail!("no chat {id}");
    }
    Ok(())
}

fn cwd_of(state: &State, id: &str) -> Option<PathBuf> {
    let draft = state.chats.get(id).and_then(|chat| chat.draft.as_ref());
    let session = state.index.find(id).map(|(_, s)| s.cwd.clone());
    session.or_else(|| draft.map(|draft| draft.cwd.clone()))
}

fn file_of(index: &Index, id: &str, draft: Option<&Draft>) -> Option<PathBuf> {
    index
        .find(id)
        .map(|(path, _)| path.to_owned())
        .or_else(|| draft?.path.clone())
}

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
