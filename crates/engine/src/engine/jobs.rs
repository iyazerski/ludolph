//! Engine-owned commands. Only a bounded output tail and recent results stay in memory.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

use super::{AlertKind, Engine, cwd_of, known};
use crate::runtime::{Runtime, kill_group};

const MAX_RUNNING: usize = 8;
const MAX_RETAINED: usize = 32;
const MAX_LOG_BYTES: usize = 32 * 1024;
const MAX_TIMEOUT: u64 = 24 * 60 * 60;
pub(super) const CUSTOM_TYPE: &str = "ludolph-job-result";

#[derive(Debug)]
pub(super) struct Jobs(Mutex<Registry>);

#[derive(Debug)]
struct Registry {
    jobs: VecDeque<Job>,
    runtime: Runtime,
    stopping: bool,
}

#[derive(Debug)]
struct Job {
    chat: String,
    info: Info,
    output: VecDeque<u8>,
    truncated: bool,
    cancel: Option<oneshot::Sender<()>>,
    settled: watch::Receiver<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    id: String,
    title: String,
    #[serde(flatten)]
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook: Option<Outcome>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum Outcome {
    Running,
    Succeeded,
    Failed { code: Option<i32> },
    TimedOut,
    Cancelled,
}

impl Outcome {
    fn description(self) -> String {
        match self {
            Self::Running => "Still running".into(),
            Self::Succeeded => "Succeeded".into(),
            Self::Failed { code: Some(code) } => format!("Failed (exit code {code})"),
            Self::Failed { code: None } => "Failed".into(),
            Self::TimedOut => "Timed out".into(),
            Self::Cancelled => "Cancelled".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Start {
        title: String,
        command: String,
        timeout_seconds: u64,
        #[serde(default)]
        on_success: Actions,
        #[serde(default)]
        on_failure: Actions,
    },
    List,
    Status {
        id: String,
    },
    Cancel {
        id: String,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Actions {
    command: Option<String>,
    #[serde(default)]
    notify_user: bool,
    resume_chat: Option<String>,
}

impl Actions {
    fn validate(&self) -> Result<()> {
        if self
            .command
            .as_ref()
            .is_some_and(|command| command.trim().is_empty() || command.len() > 64 * 1024)
        {
            bail!("a job hook must be nonempty and at most 64 KiB");
        }
        if self
            .resume_chat
            .as_ref()
            .is_some_and(|text| text.trim().is_empty() || text.len() > 2048)
        {
            bail!("provide a nonempty continuation of at most 2 KiB");
        }
        Ok(())
    }
}

impl Jobs {
    pub(super) fn open(state_dir: &Path) -> Result<Self> {
        let runtime = Runtime::recover_jobs(&state_dir.join("jobs-runtime.json"))?;
        Ok(Self(Mutex::new(Registry {
            jobs: VecDeque::new(),
            runtime,
            stopping: false,
        })))
    }

    pub(super) fn cancel_chat(&self, chat: &str) {
        let mut registry = self.0.lock().expect("jobs lock poisoned");
        for job in registry.jobs.iter_mut().filter(|job| job.chat == chat) {
            if let Some(cancel) = job.cancel.take() {
                let _ = cancel.send(());
            }
        }
    }

    pub(super) async fn shutdown(&self) {
        let pending = {
            let mut registry = self.0.lock().expect("jobs lock poisoned");
            registry.stopping = true;
            registry
                .jobs
                .iter_mut()
                .filter_map(|job| {
                    if let Some(cancel) = job.cancel.take() {
                        let _ = cancel.send(());
                    }
                    matches!(job.info.outcome, Outcome::Running).then(|| job.settled.clone())
                })
                .collect::<Vec<_>>()
        };
        for mut settled in pending {
            let _ = settled.wait_for(|done| *done).await;
        }
    }

    fn output(&self, id: &str, bytes: &[u8]) {
        let mut registry = self.0.lock().expect("jobs lock poisoned");
        let Some(job) = registry.jobs.iter_mut().find(|job| job.info.id == id) else {
            return;
        };
        let excess = (job.output.len() + bytes.len()).saturating_sub(MAX_LOG_BYTES);
        job.truncated |= excess != 0;
        job.output.drain(..excess);
        job.output.extend(bytes);
    }

    fn finish(&self, id: &str, pid: u32, outcome: Outcome, hook: Option<Outcome>) -> bool {
        let mut registry = self.0.lock().expect("jobs lock poisoned");
        if let Err(error) = registry.runtime.remove(id, pid) {
            eprintln!("job {id}: recording completion: {error:#}");
        }
        let stopping = registry.stopping;
        let Some(job) = registry.jobs.iter_mut().find(|job| job.info.id == id) else {
            return false;
        };
        let cancelled = job.cancel.take().is_none();
        (job.info.outcome, job.info.hook) = match (cancelled, hook) {
            (true, None) => (Outcome::Cancelled, None),
            (true, Some(_)) => (outcome, Some(Outcome::Cancelled)),
            (false, hook) => (outcome, hook),
        };
        !stopping && !cancelled
    }
}

impl Engine {
    pub(crate) fn background_job(&self, chat: &str, request: Request) -> Result<Value> {
        let jobs = &self.0.jobs;
        let mut registry = jobs.0.lock().expect("jobs lock poisoned");
        match request {
            Request::List => {
                let listed: Vec<_> = registry
                    .jobs
                    .iter()
                    .rev()
                    .filter(|job| job.chat == chat)
                    .map(|job| &job.info)
                    .collect();
                Ok(serde_json::to_value(listed)?)
            }
            Request::Status { id } => {
                let job = registry
                    .jobs
                    .iter_mut()
                    .find(|job| job.chat == chat && job.info.id == id)
                    .context("no retained job with that ID in this chat")?;
                Ok(serde_json::json!({
                    "job": job.info,
                    "output": String::from_utf8_lossy(job.output.make_contiguous()),
                    "truncated": job.truncated,
                }))
            }
            Request::Cancel { id } => {
                let job = registry
                    .jobs
                    .iter_mut()
                    .find(|job| job.chat == chat && job.info.id == id)
                    .context("no retained job with that ID in this chat")?;
                let requested = job
                    .cancel
                    .take()
                    .is_some_and(|cancel| cancel.send(()).is_ok());
                Ok(serde_json::json!({"job": job.info, "cancelRequested": requested}))
            }
            Request::Start {
                title,
                command,
                timeout_seconds,
                on_success,
                on_failure,
            } => {
                let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                if title.is_empty()
                    || title.chars().count() > 120
                    || command.trim().is_empty()
                    || command.len() > 64 * 1024
                {
                    bail!("provide a short title and a nonempty command of at most 64 KiB");
                }
                if !(1..=MAX_TIMEOUT).contains(&timeout_seconds) {
                    bail!("a job timeout must be between one second and 24 hours");
                }
                on_success.validate()?;
                on_failure.validate()?;
                if registry.stopping {
                    bail!("the engine is shutting down");
                }
                if registry
                    .jobs
                    .iter()
                    .filter(|job| matches!(job.info.outcome, Outcome::Running))
                    .count()
                    >= MAX_RUNNING
                {
                    bail!("too many background jobs are running");
                }
                let (cwd, stops) = {
                    let state = self.state();
                    known(&state, chat)?;
                    if state.stopping {
                        bail!("the engine is shutting down");
                    }
                    if state.chats.get(chat).is_some_and(|chat| chat.elsewhere) {
                        bail!("this chat is open in another pi");
                    }
                    let cwd = cwd_of(&state, chat).context("the chat has no project folder")?;
                    let stops = state.chats.get(chat).map_or(0, |chat| chat.stops);
                    (cwd, stops)
                };
                let info = Info {
                    id: uuid::Uuid::new_v4().to_string(),
                    title,
                    outcome: Outcome::Running,
                    hook: None,
                };
                let (mut child, pid) = spawn(&mut registry.runtime, &info.id, &cwd, &command)?;
                let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
                if registry.jobs.len() >= MAX_RETAINED
                    && let Some(oldest) = registry
                        .jobs
                        .iter()
                        .position(|job| !matches!(job.info.outcome, Outcome::Running))
                {
                    registry.jobs.remove(oldest);
                }
                let (cancel, mut cancelled) = oneshot::channel();
                let (settled, done) = watch::channel(false);
                registry.jobs.push_back(Job {
                    chat: chat.to_owned(),
                    info: info.clone(),
                    output: VecDeque::new(),
                    truncated: false,
                    cancel: Some(cancel),
                    settled: done,
                });
                let (engine, chat, task) = (self.clone(), chat.to_owned(), info.clone());
                tokio::spawn(async move {
                    let jobs = &engine.0.jobs;
                    let outcome =
                        run(jobs, &task.id, &mut child, pid, &mut cancelled, deadline).await;
                    let command = match outcome {
                        Outcome::Succeeded => on_success.command.as_deref(),
                        Outcome::Failed { .. } => on_failure.command.as_deref(),
                        _ => None,
                    };
                    let mut last_pid = pid;
                    let hook = if let Some(command) = command {
                        let started = {
                            let mut registry = jobs.0.lock().expect("jobs lock poisoned");
                            if registry
                                .jobs
                                .iter()
                                .any(|job| job.info.id == task.id && job.cancel.is_none())
                            {
                                Err(Outcome::Cancelled)
                            } else if Instant::now() >= deadline {
                                Err(Outcome::TimedOut)
                            } else {
                                spawn(&mut registry.runtime, &task.id, &cwd, command)
                                    .map_err(|error| jobs_error(&task.id, "starting hook", &error))
                            }
                        };
                        Some(match started {
                            Ok((mut child, pid)) => {
                                last_pid = pid;
                                run(jobs, &task.id, &mut child, pid, &mut cancelled, deadline).await
                            }
                            Err(outcome) => outcome,
                        })
                    } else {
                        None
                    };
                    let deliver = jobs.finish(&task.id, last_pid, outcome, hook);
                    settled.send_replace(true);
                    if !deliver
                        || matches!(outcome, Outcome::Cancelled)
                        || matches!(hook, Some(Outcome::Cancelled))
                    {
                        return;
                    }
                    let actions = if matches!(outcome, Outcome::Succeeded)
                        && matches!(hook, None | Some(Outcome::Succeeded))
                    {
                        on_success
                    } else {
                        on_failure
                    };
                    let status = match hook {
                        Some(hook) => format!(
                            "Command: {}. Hook: {}",
                            outcome.description(),
                            hook.description()
                        ),
                        None => outcome.description(),
                    };
                    if actions.notify_user {
                        engine.alert(
                            &chat,
                            AlertKind::Job {
                                id: task.id.clone(),
                                title: task.title.clone(),
                                status: status.clone(),
                            },
                        );
                    }
                    if let Some(next) = actions.resume_chat {
                        let message = format!(
                            "Ludolph background job: {}\nStatus: {status}\nJob ID: {}\nUse background_job status to read its output.\n\nContinuation requested when this job started: {next}",
                            task.title, task.id
                        );
                        engine.resume_job(&chat, stops, message);
                    }
                });
                Ok(serde_json::to_value(info)?)
            }
        }
    }
}

fn spawn(runtime: &mut Runtime, id: &str, cwd: &Path, command: &str) -> Result<(Child, u32)> {
    let child = Command::new("bash")
        .args(["-o", "pipefail", "-c", &format!("exec 2>&1\n{command}")])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .context("starting the background command")?;
    let pid = child.id().expect("a new child has a pid");
    if let Err(error) = runtime.add(id, pid) {
        kill_group(pid);
        let _ = runtime.remove(id, pid);
        return Err(error).context("recording the background command");
    }
    Ok((child, pid))
}

fn jobs_error(id: &str, action: &str, error: &anyhow::Error) -> Outcome {
    eprintln!("job {id}: {action}: {error:#}");
    Outcome::Failed { code: None }
}

async fn run(
    jobs: &Jobs,
    id: &str,
    child: &mut Child,
    pid: u32,
    cancelled: &mut oneshot::Receiver<()>,
    deadline: Instant,
) -> Outcome {
    let mut output = child.stdout.take().expect("job output is piped");
    let drain = async {
        let mut chunk = [0; 8192];
        while let Ok(n @ 1..) = output.read(&mut chunk).await {
            jobs.output(id, &chunk[..n]);
        }
    };
    let mut drain = std::pin::pin!(drain);
    let mut reading = true;
    let outcome = loop {
        tokio::select! {
            biased;
            _ = &mut *cancelled => break Outcome::Cancelled,
            _ = tokio::time::sleep_until(deadline) => break Outcome::TimedOut,
            () = &mut drain, if reading => reading = false,
            exited = child.wait() => {
                // The leader has been reaped. Do not signal its old process group after this.
                if reading {
                    let _ = tokio::time::timeout_at(deadline.min(Instant::now() + Duration::from_secs(1)), &mut drain).await;
                }
                return match exited {
                    Ok(status) if status.success() => Outcome::Succeeded,
                    Ok(status) => Outcome::Failed { code: status.code() },
                    Err(error) => jobs_error(id, "waiting for process", &error.into()),
                };
            }
        }
    };
    kill_group(pid);
    let _ = child.wait().await;
    outcome
}
