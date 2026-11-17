//! Sign-in runs pi's own `pi auth login`, which needs a terminal. macOS's `script` gives it one
//! over pipes, so Ludolph relays what pi prints and the user's answers. pi opens the browser for an
//! account sign-in itself.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, watch};

const SCRIPT: &str = "/usr/bin/script";
/// pi prints a prompt without a newline; this long a pause after one means it waits for an answer,
/// not that the rest of the line is on its way.
const SETTLE: Duration = Duration::from_millis(300);
/// The output keeps its tail, since pi prints a few lines but a URL may repeat.
const KEPT: usize = 16 * 1024;
const HIDDEN: &str = "••••";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Answers and cancels name the attempt they're for, so a late one can't reach the next.
    pub attempt: u64,
    pub provider: Option<String>,
    pub output: String,
    /// pi waits for the user to type something.
    pub waiting: bool,
    /// Whether pi saved the credentials, once it exits.
    pub signed_in: Option<bool>,
}

#[derive(Debug, Default)]
pub struct SignIn {
    inner: Arc<Mutex<Inner>>,
    revision: watch::Sender<u64>,
}

#[derive(Debug, Default)]
struct Inner {
    state: State,
    running: Option<Running>,
}

#[derive(Debug)]
struct Running {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    /// Dropping it stops the attempt. Only the task that owns the child kills it, before reaping
    /// it, so a reused pid is never signalled.
    _cancel: oneshot::Sender<()>,
    /// pi echoes some answers, such as a pasted code, so they're hidden from the output.
    answers: Arc<Mutex<Vec<String>>>,
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.lock().running.take();
    }
}

impl SignIn {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    pub fn state(&self) -> State {
        self.lock().state.clone()
    }

    /// Replaces a sign-in that is still running.
    pub fn start(
        &self,
        program: &Path,
        home: &Path,
        provider: &str,
        api_key: bool,
    ) -> Result<State> {
        let mut inner = self.lock();
        inner.running.take();
        let mut command = Command::new(SCRIPT);
        command
            .args(["-q", "/dev/null"])
            .arg(program)
            .args(["auth", "login", provider]);
        if api_key {
            command.arg("--api-key");
        }
        let mut child = command
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {SCRIPT}"))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let attempt = inner.state.attempt + 1;
        let (cancel, cancelled) = oneshot::channel();
        let answers = Arc::default();
        inner.state = State {
            attempt,
            provider: Some(provider.to_owned()),
            ..State::default()
        };
        inner.running = Some(Running {
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            _cancel: cancel,
            answers: Arc::clone(&answers),
        });
        let state = inner.state.clone();
        drop(inner);
        self.bump();
        let relay = Relay {
            inner: Arc::clone(&self.inner),
            revision: self.revision.clone(),
            attempt,
            answers,
        };
        tokio::spawn(relay.run(child, stdout, cancelled));
        Ok(state)
    }

    /// Types `text` and Return into pi's prompt.
    pub async fn answer(&self, attempt: u64, text: &str) -> Result<State> {
        let line = text.trim().replace(['\r', '\n'], "");
        let stdin = {
            let mut inner = self.lock();
            let Inner { state, running } = &mut *inner;
            let Some(running) = running.as_ref().filter(|_| state.attempt == attempt) else {
                bail!("that sign-in has ended");
            };
            if !state.waiting {
                bail!("pi isn't asking anything");
            }
            if !line.is_empty() {
                running
                    .answers
                    .lock()
                    .expect("lock poisoned")
                    .push(line.clone());
            }
            state.waiting = false;
            Arc::clone(&running.stdin)
        };
        self.bump();
        let mut stdin = stdin.lock().await;
        stdin
            .write_all(format!("{line}\r").as_bytes())
            .await
            .context("answering pi")?;
        stdin.flush().await.context("answering pi")?;
        Ok(self.state())
    }

    /// Stops `attempt` if it still runs, and clears it either way.
    pub fn cancel(&self, attempt: u64) {
        let mut inner = self.lock();
        if inner.state.attempt != attempt {
            return;
        }
        inner.running.take();
        inner.state = State {
            attempt,
            ..State::default()
        };
        drop(inner);
        self.bump();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("lock poisoned")
    }

    fn bump(&self) {
        self.revision.send_modify(|r| *r += 1);
    }
}

/// Relays one attempt's output into the state until pi exits or the attempt is cancelled.
struct Relay {
    inner: Arc<Mutex<Inner>>,
    revision: watch::Sender<u64>,
    attempt: u64,
    answers: Arc<Mutex<Vec<String>>>,
}

impl Relay {
    async fn run(self, mut child: Child, stdout: ChildStdout, cancelled: oneshot::Receiver<()>) {
        let signed_in = tokio::select! {
            // The sender is dropped when the attempt is replaced, cancelled or shut down.
            _ = cancelled => {
                if let Some(pid) = child.id() {
                    crate::runtime::kill_group(pid);
                }
                drop(child.wait().await);
                return;
            }
            status = async {
                self.read(stdout).await;
                child.wait().await
            } => status.is_ok_and(|status| status.success()),
        };
        self.update(true, |s| {
            s.waiting = false;
            s.signed_in = Some(signed_in);
        });
    }

    async fn read(&self, mut stdout: ChildStdout) {
        let mut raw = String::new();
        let mut buf = [0; 4096];
        loop {
            match tokio::time::timeout(SETTLE, stdout.read(&mut buf)).await {
                Err(_) => {
                    let waiting = !raw.ends_with('\n') && !raw.trim().is_empty();
                    self.update(false, |s| s.waiting = waiting);
                }
                Ok(Ok(0) | Err(_)) => return,
                Ok(Ok(n)) => {
                    raw.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if raw.len() > KEPT {
                        let mut cut = raw.len() - KEPT;
                        while !raw.is_char_boundary(cut) {
                            cut += 1;
                        }
                        raw.drain(..cut);
                    }
                    let mut output = crate::text::plain(&raw);
                    for answer in self.answers.lock().expect("lock poisoned").iter() {
                        output = output.replace(answer.as_str(), HIDDEN);
                    }
                    self.update(false, |s| {
                        s.output = output;
                        s.waiting = false;
                    });
                }
            }
        }
    }

    fn update(&self, exited: bool, change: impl FnOnce(&mut State)) {
        let mut inner = self.inner.lock().expect("lock poisoned");
        if inner.state.attempt != self.attempt || inner.running.is_none() {
            return;
        }
        if exited {
            inner.running.take();
        }
        change(&mut inner.state);
        drop(inner);
        self.revision.send_modify(|r| *r += 1);
    }
}

/// Removes the credentials pi stored for `provider`. Environment variables and models.json stay.
pub async fn sign_out(program: &Path, home: &Path, provider: &str) -> Result<()> {
    let output = Command::new(program)
        .args(["auth", "logout", provider])
        .current_dir(home)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("starting {}", program.display()))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        bail!("{}", crate::text::plain(why.trim()));
    }
    Ok(())
}
