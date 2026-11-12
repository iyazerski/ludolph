//! The engine's chats: which have a pi process, what they're doing, and the
//! commands that start, steer and stop their runs.
//!
//! A chat is a pi session, named by its session id. Its pi starts when the
//! chat first needs one and stops after a few idle minutes (`docs/design.md`,
//! "pi processes").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::{OwnedMutexGuard, mpsc, watch};
use tokio::time::Instant;

use crate::index::Index;
use crate::live::Live;
use crate::pi::{Event, Options, Pi, Session};
use crate::runtime::Runtime;
use crate::session::{Outcome, Summary};

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
}

#[derive(Debug)]
struct State {
    index: Index,
    runtime: Runtime,
    chats: HashMap<String, Chat>,
}

/// What the engine knows about a chat beyond its session file.
#[derive(Debug, Default)]
struct Chat {
    /// Held by commands that start or stop the chat's pi, so they don't overlap.
    ops: Arc<tokio::sync::Mutex<()>>,
    /// A new chat whose session file pi hasn't written yet.
    draft: Option<Draft>,
    process: Option<Running>,
    /// pi or the engine stopped during the last run.
    interrupted: bool,
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
    /// The file's leaf when the current run started: the live tail stands in
    /// for everything after it.
    base: Option<String>,
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
    pub status: Status,
}

/// What a chat is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working,
    /// The last run ended on an error.
    Failed,
    /// pi or the engine stopped during the last run.
    Interrupted,
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

impl Engine {
    /// Start the engine: stop the pi processes a previous engine left
    /// running, and bring the index up to date.
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
            index.save(&index_path)?;
            Ok(index)
        })
        .await??;
        let chats = interrupted
            .into_iter()
            .map(|id| {
                let chat = Chat {
                    interrupted: true,
                    ..Chat::default()
                };
                (id, chat)
            })
            .collect();
        let state = State {
            index,
            runtime,
            chats,
        };
        Ok(Self(Arc::new(Inner {
            config,
            state: Mutex::new(state),
            list: watch::channel(0).0,
        })))
    }

    /// Every chat, most recently active first.
    pub fn chats(&self) -> Vec<ChatInfo> {
        let state = self.state();
        let mut chats: Vec<ChatInfo> = state
            .index
            .iter()
            .map(|(_, summary)| {
                let chat = state.chats.get(&summary.id);
                ChatInfo {
                    id: summary.id.clone(),
                    cwd: summary.cwd.clone(),
                    created: summary.created,
                    status: status(chat, Some(summary)),
                    summary: Some(summary.clone()),
                }
            })
            .collect();
        let drafts = state.chats.iter().filter_map(|(id, chat)| {
            let draft = chat.draft.as_ref()?;
            let written = draft.path.as_deref().and_then(|path| state.index.get(path));
            written.is_none().then(|| ChatInfo {
                id: id.clone(),
                cwd: draft.cwd.clone(),
                created: draft.created,
                summary: None,
                status: status(Some(chat), None),
            })
        });
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

    /// Send `text` to a chat, with images as pi's `ImageContent` values. It
    /// starts a run, or steers the one that's going.
    ///
    /// # Errors
    ///
    /// Fails if there's no such chat, its folder is gone, or pi refuses the
    /// message.
    pub async fn send(&self, id: &str, text: &str, images: Vec<Value>) -> Result<Delivery> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        let pi = self.process(id)?;
        {
            let mut state = self.state();
            let State { index, chats, .. } = &mut *state;
            let chat = chats.get_mut(id).expect("a chat with a process is known");
            let path = file_of(index, id, chat);
            let running = chat
                .process
                .as_mut()
                .expect("the process was just looked up");
            if !running.live.working() {
                if let Some(path) = &path {
                    index.refresh_file(path)?;
                }
                running.base = path.and_then(|path| index.get(&path)?.leaf.clone());
                chat.interrupted = false;
            }
        }
        // pi ignores `steer` when no run is going, so the answer says which
        // happened, even if a run started or ended meanwhile.
        let mut command = json!({"type": "prompt", "message": text, "streamingBehavior": "steer"});
        if !images.is_empty() {
            command["images"] = Value::Array(images);
        }
        let answer = pi.call(command).await?;
        match answer["disposition"].as_str() {
            Some("started") => Ok(Delivery::Started),
            Some("queued") => Ok(Delivery::Queued),
            Some("handled") => Ok(Delivery::Handled),
            other => bail!("pi answered the prompt with {other:?}"),
        }
    }

    /// Stop a chat's run: drop its queued messages, then abort.
    ///
    /// # Errors
    ///
    /// Fails if pi refuses.
    pub async fn interrupt(&self, id: &str) -> Result<()> {
        let pi = {
            let state = self.state();
            let running = state.chats.get(id).and_then(|chat| chat.process.as_ref());
            match running {
                Some(running) => running.pi.clone(),
                None => return Ok(()),
            }
        };
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
        let processes: Vec<Arc<Pi>> = {
            let state = self.state();
            let running = state
                .chats
                .values()
                .filter_map(|chat| chat.process.as_ref());
            running.map(|running| running.pi.clone()).collect()
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
        self.save_index()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().expect("lock poisoned")
    }

    /// The lock that serializes a chat's commands.
    fn ops(&self, id: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let mut state = self.state();
        if !state.chats.contains_key(id) && !state.index.iter().any(|(_, s)| s.id == id) {
            bail!("no chat {id}");
        }
        Ok(state.chats.entry(id.to_owned()).or_default().ops.clone())
    }

    /// The chat's pi, started if it has none. Callers hold the chat's `ops`.
    fn process(&self, id: &str) -> Result<Arc<Pi>> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
        } = &mut *state;
        let chat = chats.get_mut(id).expect("callers look the chat up first");
        if let Some(running) = &chat.process {
            return Ok(running.pi.clone());
        }
        let (cwd, session) = match (index.iter().find(|(_, s)| s.id == id), &chat.draft) {
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
            base: None,
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
        } = &mut *state;
        let Some(chat) = chats.get_mut(id) else {
            return Ok(());
        };
        let Some(running) = chat.process.as_mut().filter(|r| Arc::ptr_eq(&r.pi, pi)) else {
            return Ok(());
        };
        let working = running.live.working();
        running.live.apply(event);
        let mut list_changed = running.live.working() != working;
        match event {
            Event::AgentStart => runtime.set_running(id, true)?,
            Event::AgentSettled { .. } => {
                runtime.set_running(id, false)?;
                running.base = None;
                if let Some(path) = file_of(index, id, chat) {
                    index.refresh_file(&path)?;
                }
                // The file has the run's messages now.
                if let Some(running) = chat.process.as_mut() {
                    running.live.clear_messages();
                }
                list_changed = true;
            }
            Event::SessionInfoChanged { .. } => {
                if let Some(path) = file_of(index, id, chat) {
                    index.refresh_file(&path)?;
                }
                list_changed = true;
            }
            _ => {}
        }
        let settled = matches!(event, Event::AgentSettled { .. });
        drop(state);
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
            running.is_some_and(|r| r.live.working())
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

    /// Forget `pi` as its chat's process, if it still is, and remember a run
    /// it cut short.
    fn detach(&self, id: &str, pi: &Arc<Pi>) -> Result<()> {
        let mut state = self.state();
        let State {
            index,
            runtime,
            chats,
        } = &mut *state;
        runtime.remove(id, pi.pid())?;
        let Some(chat) = chats.get_mut(id) else {
            return Ok(());
        };
        let Some(running) = chat.process.take_if(|r| Arc::ptr_eq(&r.pi, pi)) else {
            return Ok(());
        };
        if running.live.working() {
            chat.interrupted = true;
        }
        if let Some(path) = file_of(index, id, chat) {
            index.refresh_file(&path)?;
        }
        Ok(())
    }

    fn save_index(&self) -> Result<()> {
        let path = self.0.config.state_dir.join("index.json");
        self.state().index.save(&path)
    }

    fn changed(&self) {
        self.0.list.send_modify(|revision| *revision += 1);
    }
}

/// The chat's session file: the index's, or the one pi named for a draft.
fn file_of(index: &Index, id: &str, chat: &Chat) -> Option<PathBuf> {
    index
        .iter()
        .find(|(_, summary)| summary.id == id)
        .map(|(path, _)| path.to_owned())
        .or_else(|| chat.draft.as_ref()?.path.clone())
}

/// What a chat is doing, from its process if it has one, else from how its
/// last run ended in the file.
fn status(chat: Option<&Chat>, summary: Option<&Summary>) -> Status {
    if let Some(live) = chat.and_then(|chat| chat.process.as_ref()).map(|r| &r.live) {
        if live.working() {
            return Status::Working;
        }
        if live.failure().is_some() {
            return Status::Failed;
        }
    }
    if chat.is_some_and(|chat| chat.interrupted) {
        return Status::Interrupted;
    }
    match summary.and_then(|summary| summary.outcome) {
        Some(Outcome::Failed) => Status::Failed,
        _ => Status::Idle,
    }
}
