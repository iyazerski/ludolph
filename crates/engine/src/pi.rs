//! One pi process in RPC mode (pi's `docs/rpc.md`): commands and their
//! responses, matched by id, and the events it streams.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, watch};

use crate::jsonl::fix_surrogates;

/// How many events wait for the engine before pi has to wait. pi honors
/// stdout backpressure, so a slow reader slows pi rather than growing memory.
const EVENT_QUEUE: usize = 256;

/// How long [`Pi::ask`] waits for its answer. pi loads its extensions
/// first, and one could hang.
const ASK_TIMEOUT: Duration = Duration::from_secs(30);

/// Which session a process opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// A new session with this id, created in the process's folder.
    New(String),
    /// The session in this file.
    Resume(PathBuf),
    /// No session file at all, for a pi that only answers questions.
    None,
}

/// How to start a pi process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The `pi` executable.
    pub program: PathBuf,
    /// The folder pi runs in, which is the chat's project.
    pub cwd: PathBuf,
    pub session: Session,
    /// Extensions to load with `--extension`, on top of pi's own.
    pub extensions: Vec<PathBuf>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
}

/// A running pi. Dropping it kills its process group; [`Pi::stop`] stops
/// it cleanly.
#[derive(Debug)]
pub struct Pi {
    pid: u32,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    /// The exit status once pi has exited; `None` inside if it couldn't be read.
    exit: watch::Receiver<Option<Option<ExitStatus>>>,
}

/// Calls waiting for their response, by id. The reader takes the map when pi
/// exits, which fails the calls still waiting and any later ones.
type Pending = Arc<Mutex<Option<HashMap<String, oneshot::Sender<Response>>>>>;

#[derive(Debug, Deserialize)]
struct Response {
    id: Option<String>,
    success: bool,
    data: Option<Value>,
    error: Option<String>,
}

/// An event pi streams (pi's `docs/json.md` and `docs/rpc-extension-ui.md`),
/// with the ones the engine doesn't use as `Other`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum Event {
    AgentStart,
    /// pi has no more automatic work for the run.
    AgentSettled {
        aborted: bool,
    },
    MessageStart {
        message: Value,
    },
    /// One content-block change to the streaming assistant message.
    MessageUpdate {
        assistant_message_event: Value,
    },
    /// The final message, which replaces the streamed one.
    MessageEnd {
        message: Value,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        parent_tool_call_id: Option<String>,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        partial_result: Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        is_error: bool,
        parent_tool_call_id: Option<String>,
        duration_ms: Option<f64>,
        #[serde(default)]
        result: Value,
    },
    /// The whole steering and follow-up queue.
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    SessionInfoChanged {
        name: Option<String>,
    },
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: Option<u64>,
        error_message: Option<String>,
    },
    AutoRetryEnd {
        success: bool,
        final_error: Option<String>,
    },
    CompactionStart {
        /// `manual`, `threshold` or `overflow`.
        reason: Option<String>,
    },
    CompactionEnd {
        error_message: Option<String>,
    },
    /// An extension asking something, or telling something (`notify` and
    /// the other fire-and-forget methods).
    ExtensionUiRequest {
        id: String,
        method: String,
        title: Option<String>,
        message: Option<String>,
        options: Option<Vec<String>>,
        placeholder: Option<String>,
        prefill: Option<String>,
        timeout: Option<u64>,
    },
    #[serde(other)]
    Other,
}

impl Pi {
    /// Start pi in RPC mode, and return it with its events.
    ///
    /// The events end when pi exits. pi runs in its own process group, so
    /// [`Pi::stop`] can stop the tools it started with it. Its stderr goes to
    /// the engine's.
    ///
    /// # Errors
    ///
    /// Fails if pi can't be started.
    pub fn start(options: &Options) -> Result<(Self, mpsc::Receiver<Event>)> {
        let mut command = Command::new(&options.program);
        command.args(["--mode", "rpc"]);
        match &options.session {
            Session::New(id) => command.arg("--session-id").arg(id),
            Session::Resume(file) => command.arg("--session").arg(file),
            Session::None => command.arg("--no-session"),
        };
        for extension in &options.extensions {
            command.arg("--extension").arg(extension);
        }
        let mut child = command
            .envs(options.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&options.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting {}", options.program.display()))?;
        let pid = child.id().expect("a child that was just started has a pid");
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (events, receiver) = mpsc::channel(EVENT_QUEUE);
        tokio::spawn(read(stdout, pending.clone(), events));
        let (exited, exit) = watch::channel(None);
        tokio::spawn(reap(child, exited));
        let pi = Self {
            pid,
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            pending,
            next_id: AtomicU64::new(1),
            exit,
        };
        Ok((pi, receiver))
    }

    /// Start pi without a session in `cwd`, send it one command, stop it,
    /// and return the command's answer. It's for questions that need no chat,
    /// like which models or skills there are.
    ///
    /// # Errors
    ///
    /// Fails if pi can't be started, refuses the command, or doesn't answer
    /// in time.
    pub async fn ask(program: &Path, cwd: &Path, grace: Duration, command: Value) -> Result<Value> {
        let mut answers = Self::ask_all(program, cwd, grace, vec![command]).await?;
        Ok(answers.remove(0))
    }

    /// [`Pi::ask`] with several commands, one after another, for one pi's
    /// start-up cost. Returns their answers in order.
    ///
    /// # Errors
    ///
    /// Fails if pi can't be started, refuses a command, or doesn't answer
    /// them all in time.
    pub async fn ask_all(
        program: &Path,
        cwd: &Path,
        grace: Duration,
        commands: Vec<Value>,
    ) -> Result<Vec<Value>> {
        let options = Options {
            program: program.to_owned(),
            cwd: cwd.to_owned(),
            session: Session::None,
            extensions: Vec::new(),
            env: Vec::new(),
        };
        let (pi, _events) = Self::start(&options)?;
        let calls = async {
            let mut answers = Vec::with_capacity(commands.len());
            for command in commands {
                answers.push(pi.call(command).await?);
            }
            Ok(answers)
        };
        let answer = tokio::time::timeout(ASK_TIMEOUT, calls).await;
        if let Err(e) = pi.stop(grace).await {
            eprintln!("stopping a pi that answered a question: {e:#}");
        }
        answer.map_err(|_| anyhow!("pi didn't answer in {ASK_TIMEOUT:?}"))?
    }

    /// The process id, which is also its process group id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Send a command, such as `{"type": "get_state"}`, and return its
    /// response's `data` (`null` if it has none).
    ///
    /// # Errors
    ///
    /// Fails with pi's error if pi refuses the command, and fails if pi exits
    /// before it answers.
    pub async fn call(&self, mut command: Value) -> Result<Value> {
        let id = format!("l{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let kind = command["type"].as_str().unwrap_or("command").to_owned();
        command["id"] = json!(id);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("lock poisoned")
            .as_mut()
            .ok_or_else(|| anyhow!("pi exited"))?
            .insert(id.clone(), sender);
        if let Err(e) = self.send(&command).await {
            if let Some(pending) = self.pending.lock().expect("lock poisoned").as_mut() {
                pending.remove(&id);
            }
            return Err(e);
        }
        let response = receiver
            .await
            .map_err(|_| anyhow!("pi exited before answering {kind}"))?;
        if !response.success {
            bail!("pi refused {kind}: {}", response.error.unwrap_or_default());
        }
        Ok(response.data.unwrap_or(Value::Null))
    }

    /// Write one record that gets no response, such as an
    /// `extension_ui_response`.
    ///
    /// # Errors
    ///
    /// Fails if pi has exited or is being stopped.
    pub async fn send(&self, record: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin.as_mut().ok_or_else(|| anyhow!("pi is stopping"))?;
        stdin.write_all(&line).await.context("writing to pi")?;
        stdin.flush().await.context("writing to pi")
    }

    /// Stop pi: close its stdin, which asks it to finish and exit, and kill
    /// its process group if it hasn't exited after `grace`.
    ///
    /// # Errors
    ///
    /// Fails if waiting for the process fails.
    pub async fn stop(&self, grace: Duration) -> Result<ExitStatus> {
        self.stdin.lock().await.take();
        if let Ok(status) = tokio::time::timeout(grace, self.wait()).await {
            return status;
        }
        crate::runtime::kill_group(self.pid);
        self.wait().await
    }

    /// Wait for pi to exit.
    ///
    /// # Errors
    ///
    /// Fails if pi's exit status couldn't be read.
    pub async fn wait(&self) -> Result<ExitStatus> {
        let mut exit = self.exit.clone();
        let status = exit
            .wait_for(Option::is_some)
            .await
            .context("waiting for pi")?;
        status.flatten().context("reading pi's exit status")
    }
}

impl Drop for Pi {
    fn drop(&mut self) {
        if self.exit.borrow().is_none() {
            crate::runtime::kill_group(self.pid);
        }
    }
}

/// Wait for pi to exit, stop its tools if it crashed, and publish its
/// status.
async fn reap(mut child: Child, exited: watch::Sender<Option<Option<ExitStatus>>>) {
    let pid = child.id();
    let status = child.wait().await;
    if let Err(e) = &status {
        eprintln!("waiting for pi: {e}");
    }
    // A crashed pi's tools would keep changing files with no one watching.
    // A clean exit leaves the jobs the agent started on purpose, such as a
    // dev server, as a terminal pi does. Right away, so the group's id
    // can't have been reused yet.
    if let Some(pid) = pid
        && !status.as_ref().is_ok_and(ExitStatus::success)
    {
        crate::runtime::kill_group(pid);
    }
    exited.send_replace(Some(status.ok()));
}

/// Read pi's stdout until it closes: responses go to their calls, and events
/// to `events`.
async fn read(stdout: ChildStdout, pending: Pending, events: mpsc::Sender<Event>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("reading pi's output: {e}");
                break;
            }
        }
        // Records end with LF, maybe after a CR (`docs/rpc.md`).
        fix_surrogates(&mut line);
        let record = line.strip_suffix(b"\n").unwrap_or(&line);
        let record = record.strip_suffix(b"\r").unwrap_or(record);
        let value: Value = match serde_json::from_slice(record) {
            Ok(value) => value,
            Err(e) => {
                eprintln!("skipping a record from pi that isn't JSON: {e}");
                continue;
            }
        };
        if value["type"] == "response" {
            respond(&pending, value);
            continue;
        }
        match serde_json::from_value::<Event>(value) {
            Ok(event) => {
                if events.send(event).await.is_err() {
                    // Nobody listens, but responses still need reading.
                    continue;
                }
            }
            Err(e) => eprintln!("skipping an event from pi: {e}"),
        }
    }
    pending.lock().expect("lock poisoned").take();
}

/// Hand a response to the call waiting for it.
fn respond(pending: &Pending, value: Value) {
    let response: Response = match serde_json::from_value(value) {
        Ok(response) => response,
        Err(e) => return eprintln!("skipping a response from pi: {e}"),
    };
    let Some(id) = &response.id else {
        // Only a command pi couldn't parse has no id.
        return eprintln!(
            "pi couldn't parse a command: {}",
            response.error.unwrap_or_default()
        );
    };
    let waiting = pending
        .lock()
        .expect("lock poisoned")
        .as_mut()
        .and_then(|pending| pending.remove(id));
    if let Some(waiting) = waiting {
        // The call may have been dropped; then nobody needs the answer.
        let _ = waiting.send(response);
    }
}
