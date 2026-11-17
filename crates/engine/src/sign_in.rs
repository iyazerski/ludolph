//! Sign-in runs pi's own login in a pi with `pi-setup.js`, which reports pi's steps as notifications
//! and asks pi's questions as dialogs. That pi has no terminal or browser, so Ludolph opens pages.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};

use crate::pi::{Event, Pi};
use crate::pi_setup;

/// How long a sign-in may wait on the user before it gives up.
const TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Cancels name the attempt they're for, so a late one can't stop the next.
    pub attempt: u64,
    pub provider: Option<String>,
    /// The page pi signs in on, already opened in the browser.
    pub browser: Option<String>,
    pub device_code: Option<DeviceCode>,
    pub prompt: Option<Prompt>,
    /// Something pi says to do outside Ludolph, such as a command to run.
    pub note: Option<String>,
    /// How the sign-in ended, once it has.
    pub result: Option<Outcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub code: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    /// pi's dialog, which the answer names.
    pub id: String,
    pub kind: PromptKind,
    pub message: String,
    pub placeholder: Option<String>,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Text,
    Secret,
    Select,
    /// A code pasted from the browser, which pi's own callback usually makes unneeded.
    ManualCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub signed_in: bool,
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    ludolph_sign_in: StepBody,
}

#[derive(Deserialize)]
struct StepBody {
    event: Option<AuthEvent>,
    prompt: Option<AskedPrompt>,
    /// pi dropped its question, as when the browser callback beats a pasted code.
    #[serde(default)]
    ended: bool,
}

#[derive(Deserialize)]
struct AskedPrompt {
    #[serde(rename = "type")]
    kind: PromptKind,
    message: String,
}

/// pi's `AuthEvent`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AuthEvent {
    AuthUrl {
        url: String,
    },
    DeviceCode {
        #[serde(rename = "userCode")]
        user_code: String,
        #[serde(rename = "verificationUri")]
        verification_uri: String,
        #[serde(rename = "openBrowserOnConfirm")]
        open_browser: Option<bool>,
    },
    Info {
        message: String,
    },
    #[serde(other)]
    Other,
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
    pi: Arc<Pi>,
    /// Dropping it stops the attempt.
    _stop: oneshot::Sender<()>,
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
        extensions: &Path,
        provider: &str,
        api_key: bool,
    ) -> Result<State> {
        let mut inner = self.lock();
        inner.running.take();
        let (pi, events) = pi_setup::start(program, extensions)?;
        let pi = Arc::new(pi);
        let attempt = inner.state.attempt + 1;
        let (stop, stopped) = oneshot::channel();
        inner.state = State {
            attempt,
            provider: Some(provider.to_owned()),
            ..State::default()
        };
        inner.running = Some(Running {
            pi: Arc::clone(&pi),
            _stop: stop,
        });
        let state = inner.state.clone();
        drop(inner);
        self.bump();
        let request = json!({"topic": "signIn", "provider": provider, "apiKey": api_key});
        let relay = Relay {
            inner: Arc::clone(&self.inner),
            revision: self.revision.clone(),
            attempt,
        };
        tokio::spawn(relay.run(pi, events, pi_setup::command(&request), stopped));
        Ok(state)
    }

    /// Answers pi's question `prompt` of the running sign-in.
    pub async fn answer(&self, prompt: &str, value: &str) -> Result<State> {
        let (pi, answer) = {
            let mut inner = self.lock();
            let Inner { state, running } = &mut *inner;
            let (Some(running), Some(asked)) = (running.as_ref(), state.prompt.as_ref()) else {
                bail!("pi isn't asking anything");
            };
            if asked.id != prompt {
                bail!("pi isn't asking that anymore");
            }
            let answer = match asked.kind {
                PromptKind::Select if !asked.options.iter().any(|o| o == value) => {
                    bail!("{value} isn't one of pi's choices")
                }
                PromptKind::Select => value.to_owned(),
                _ => value.trim().to_owned(),
            };
            state.prompt = None;
            (Arc::clone(&running.pi), answer)
        };
        self.bump();
        pi.send(&json!({"type": "extension_ui_response", "id": prompt, "value": answer}))
            .await?;
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

/// Turns one attempt's pi into its state until pi answers or the attempt stops.
struct Relay {
    inner: Arc<Mutex<Inner>>,
    revision: watch::Sender<u64>,
    attempt: u64,
}

impl Relay {
    async fn run(
        self,
        pi: Arc<Pi>,
        mut events: mpsc::Receiver<Event>,
        command: serde_json::Value,
        stopped: oneshot::Receiver<()>,
    ) {
        let work = async {
            // The command's reply comes only once the sign-in ends, and pi-setup.js says how first.
            let call = pi.call(command);
            tokio::pin!(call);
            let mut called = false;
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        None => bail!("pi exited before it signed in"),
                        Some(event) => if let Some(answer) = pi_setup::answer(&event) {
                            return answer.map(drop);
                        } else {
                            self.step(event);
                        },
                    },
                    called_back = &mut call, if !called => {
                        called = true;
                        called_back?;
                    }
                }
            }
        };
        let outcome = tokio::select! {
            // The sender is dropped when the attempt is replaced, cancelled or shut down.
            _ = stopped => None,
            outcome = tokio::time::timeout(TIMEOUT, work) => Some(match outcome {
                Ok(Ok(())) => Outcome { signed_in: true, error: None },
                Ok(Err(e)) => Outcome { signed_in: false, error: Some(format!("{e:#}")) },
                Err(_) => Outcome { signed_in: false, error: Some("It took too long.".to_owned()) },
            }),
        };
        if let Some(outcome) = outcome {
            self.update(true, |s| {
                s.prompt = None;
                s.result = Some(outcome);
            });
        }
        if let Err(e) = pi.stop(Duration::ZERO).await {
            eprintln!("stopping the pi that signed in: {e:#}");
        }
    }

    fn step(&self, event: Event) {
        let Event::ExtensionUiRequest {
            id,
            method,
            title,
            message,
            options,
            placeholder,
            ..
        } = event
        else {
            return;
        };
        let text = if method == "notify" { message } else { title };
        let Some(body) = text
            .and_then(|t| serde_json::from_str::<Step>(&t).ok())
            .map(|s| s.ludolph_sign_in)
        else {
            return;
        };
        if let Some(asked) = body.prompt {
            let prompt = Prompt {
                id,
                kind: asked.kind,
                message: asked.message,
                placeholder,
                options: options.unwrap_or_default(),
            };
            self.update(false, |s| s.prompt = Some(prompt));
            return;
        }
        if body.ended {
            self.update(false, |s| s.prompt = None);
            return;
        }
        match body.event {
            Some(AuthEvent::AuthUrl { url }) => {
                // Only the current attempt opens pages, not one cancelled while this was queued.
                if self.update(false, |s| s.browser = Some(url.clone())) {
                    open(&url);
                }
            }
            Some(AuthEvent::DeviceCode {
                user_code,
                verification_uri,
                open_browser,
            }) => {
                let code = DeviceCode {
                    code: user_code,
                    url: verification_uri.clone(),
                };
                let current = self.update(false, |s| s.device_code = Some(code));
                if current && open_browser != Some(false) {
                    open(&verification_uri);
                }
            }
            Some(AuthEvent::Info { message }) => {
                self.update(false, |s| s.note = Some(message));
            }
            Some(AuthEvent::Other) | None => {}
        }
    }

    /// Changes the state if this is still the current attempt, and says whether it was.
    fn update(&self, ended: bool, change: impl FnOnce(&mut State)) -> bool {
        let mut inner = self.inner.lock().expect("lock poisoned");
        if inner.state.attempt != self.attempt || inner.running.is_none() {
            return false;
        }
        if ended {
            inner.running.take();
        }
        change(&mut inner.state);
        drop(inner);
        self.revision.send_modify(|r| *r += 1);
        true
    }
}

/// Opens a sign-in page in the user's browser. pi gives https pages and local callbacks.
fn open(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        eprintln!("not opening a sign-in page that isn't on the web: {url}");
        return;
    }
    let mut open = tokio::process::Command::new("/usr/bin/open");
    open.arg(url);
    tokio::spawn(async move {
        if let Err(e) = open.status().await {
            eprintln!("opening a sign-in page: {e}");
        }
    });
}
