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

/// Backpressure stalls pi's stdout rather than growing the engine's memory.
const EVENT_QUEUE: usize = 256;

const ASK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    New(String),
    Resume(PathBuf),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub program: PathBuf,
    pub cwd: PathBuf,
    pub session: Session,
    pub extensions: Vec<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// Dropping pi kills its process group; `stop` permits a clean exit first.
#[derive(Debug)]
pub struct Pi {
    pid: u32,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    exit: watch::Receiver<Option<Option<ExitStatus>>>,
}

/// Taking the map on exit also rejects calls arriving after the reader has stopped.
type Pending = Arc<Mutex<Option<HashMap<String, oneshot::Sender<Response>>>>>;

#[derive(Debug, Deserialize)]
struct Response {
    id: Option<String>,
    success: bool,
    data: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum Event {
    AgentStart,
    AgentSettled {
        aborted: bool,
    },
    MessageStart {
        message: Value,
    },
    MessageUpdate {
        assistant_message_event: Value,
    },
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
        reason: Option<String>,
    },
    CompactionEnd {
        error_message: Option<String>,
    },
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

    pub async fn ask(program: &Path, cwd: &Path, grace: Duration, command: Value) -> Result<Value> {
        let mut answers = Self::ask_all(program, cwd, grace, vec![command]).await?;
        Ok(answers.remove(0))
    }

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

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn call(&self, command: Value) -> Result<Value> {
        self.call_checked(command, || Ok(())).await
    }

    /// Check cancellation under the stdin lock immediately before writing.
    pub(crate) async fn call_checked(
        &self,
        mut command: Value,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<Value> {
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
        if let Err(e) = self.send_checked(&command, check).await {
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

    pub async fn send(&self, record: &Value) -> Result<()> {
        self.send_checked(record, || Ok(())).await
    }

    async fn send_checked(&self, record: &Value, check: impl FnOnce() -> Result<()>) -> Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin.as_mut().ok_or_else(|| anyhow!("pi is stopping"))?;
        check()?;
        stdin.write_all(&line).await.context("writing to pi")?;
        stdin.flush().await.context("writing to pi")
    }

    pub async fn stop(&self, grace: Duration) -> Result<ExitStatus> {
        self.stdin.lock().await.take();
        if let Ok(status) = tokio::time::timeout(grace, self.wait()).await {
            return status;
        }
        crate::runtime::kill_group(self.pid);
        self.wait().await
    }

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

async fn reap(mut child: Child, exited: watch::Sender<Option<Option<ExitStatus>>>) {
    let pid = child.id();
    let status = child.wait().await;
    if let Err(e) = &status {
        eprintln!("waiting for pi: {e}");
    }
    // Kill crashed tools before their process-group id can be reused; clean exits may leave dev servers.
    if let Some(pid) = pid
        && !status.as_ref().is_ok_and(ExitStatus::success)
    {
        crate::runtime::kill_group(pid);
    }
    exited.send_replace(Some(status.ok()));
}

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
        let _ = waiting.send(response);
    }
}
