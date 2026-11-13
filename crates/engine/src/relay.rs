//! The link to the relay, which brings the phone's calls to this Mac
//! (`docs/design.md`, "Phone and relay").
//!
//! The engine keeps one outgoing WebSocket to the relay, and answers its
//! calls with the same tools the app uses. Only those tools exist, so the
//! relay can't reach anything else on the Mac. It also passes the chats'
//! alerts on, so the phone can notify you.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use ludolph_relay_proto::{CONNECT_PATH, Call, Engine as State, Notice, NoticeKind, PROTOCOL, Up};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode, header};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::engine::{Alert, AlertKind, Engine};
use crate::text::one_line;
use crate::tools::{self, Tool};
use crate::transcript::{self, Part};

/// How often to ping the relay. Proxies drop idle sockets, and any frame
/// back proves the link still works.
const PING_EVERY: Duration = Duration::from_secs(25);
/// Reconnect if nothing came from the relay for this long.
const SILENCE_LIMIT: Duration = Duration::from_secs(70);
/// The first wait before reconnecting. It doubles after each quick failure.
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A connection that lasted this long worked, so the next reconnect starts
/// fast again. A shorter one, like a relay refusing the protocol right
/// after hello, keeps backing off.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// How long a call may run: under the relay's 30 s, so the error still
/// reaches the phone. A call that timed out may still have happened.
const CALL_TIMEOUT: Duration = Duration::from_secs(25);
/// Calls running at once; more wait.
const MAX_CALLS: usize = 16;
/// Calls running or waiting on one connection; more are refused.
const MAX_QUEUED_CALLS: usize = 64;
/// The Mac counts as in use if it had keyboard or mouse input this recently.
const AWAY_AFTER: Duration = Duration::from_secs(120);
/// How long building one notice (reading its chat) may take.
const NOTICE_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_CHARS: usize = 240;

/// When the phone gets notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyWhen {
    /// Only while nobody uses the Mac: at it, you see the chat yourself.
    Away,
    /// Whether or not someone uses the Mac.
    Always,
}

/// The relay's address and this Mac's token, from the phone's "Add a Mac".
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// `https://…`, or `http://…` for a relay on this Mac.
    pub relay: String,
    pub token: String,
}

// By hand, so logs never show the token.
impl fmt::Debug for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Link")
            .field("relay", &self.relay)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Link {
    /// Where the link is kept in the engine's folder.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("relay.json")
    }

    /// The saved link, if this Mac has one.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read.
    pub fn load(state_dir: &Path) -> Result<Option<Self>> {
        ludolph_json_file::load(&Self::path(state_dir))
    }

    /// Save the link, readable only by you, since the token is a secret.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be written.
    pub fn save(&self, state_dir: &Path) -> Result<()> {
        ludolph_json_file::save(&Self::path(state_dir), self)
    }
}

/// Stay connected to the relay, answer its calls, and pass alerts on as
/// `notify` allows, reconnecting when the link drops. It returns only when
/// the relay rejects the token, since trying again can't help then.
pub async fn run(engine: Engine, link: Link, notify: NotifyWhen) {
    let url = match socket_url(&link.relay) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("relay: {e:#}");
            return;
        }
    };
    let slots = Arc::new(Semaphore::new(MAX_CALLS));
    let mut backoff = MIN_BACKOFF;
    loop {
        let mut connected = None;
        let result = async {
            let socket = dial(&url, &link.token).await?;
            eprintln!("relay: connected to {url}");
            connected = Some(Instant::now());
            serve(socket, &engine, &slots, notify).await
        }
        .await;
        if connected.is_some_and(|at| at.elapsed() >= STABLE_AFTER) {
            backoff = MIN_BACKOFF;
        }
        match result {
            Ok(()) => eprintln!("relay: the relay closed the link; reconnecting"),
            Err(e) if rejected(&e) => {
                eprintln!("relay: the relay rejected this Mac's token; add the Mac again");
                return;
            }
            Err(e) => eprintln!("relay: {e:#}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Check that the relay takes `link`'s token, then disconnect.
///
/// # Errors
///
/// Fails if the relay can't be reached or rejects the token.
pub async fn verify(link: &Link) -> Result<()> {
    let socket = match dial(&socket_url(&link.relay)?, &link.token).await {
        Err(e) if rejected(&e) => bail!("the relay rejected this token; add the Mac again"),
        other => other.with_context(|| format!("reaching {}", link.relay))?,
    };
    // Saying hello makes it a short visit in the relay's log, not a bad client.
    let (mut sink, _) = socket.split();
    sink.send(encode(&hello())?).await?;
    sink.close().await?;
    Ok(())
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Serve one connection until it ends. `Ok` means the relay closed it.
///
/// Alerts from before it began are dropped: a late notice would only be
/// noise (`docs/design.md`, "Phone and relay").
async fn serve(
    socket: Socket,
    engine: &Engine,
    slots: &Arc<Semaphore>,
    notify: NotifyWhen,
) -> Result<()> {
    let mut alerts = engine.alerts();
    let (mut sink, mut stream) = socket.split();
    sink.send(encode(&hello())?).await?;
    let running = State::Running {
        running: Some(env!("CARGO_PKG_VERSION").to_owned()),
    };
    sink.send(encode(&Up::Engine(running))?).await?;
    // Calls and notices being built. Dropping the set when the connection
    // ends stops them, since they have nowhere to go.
    let mut calls: JoinSet<Option<Up>> = JoinSet::new();
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            frame = stream.next() => {
                heard = Instant::now();
                match frame {
                    Some(Ok(Message::Text(body))) => match serde_json::from_str::<Call>(&body) {
                        Ok(call) if calls.len() >= MAX_QUEUED_CALLS => {
                            let busy = Up::Reply { id: call.id, result: Err("this Mac is busy; try again".into()) };
                            sink.send(encode(&busy)?).await?;
                        }
                        Ok(call) => {
                            let reply = answer(engine.clone(), slots.clone(), call);
                            calls.spawn(async { Some(reply.await) });
                        }
                        // The relay times out the call it holds.
                        Err(e) => eprintln!("relay: skipped a bad call: {e}"),
                    },
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            alert = alerts.recv() => match alert {
                Ok(alert) => _ = calls.spawn(tell(engine.clone(), alert, notify)),
                Err(RecvError::Lagged(missed)) => eprintln!("relay: skipped {missed} notifications"),
                Err(RecvError::Closed) => bail!("the engine stopped"),
            },
            Some(Ok(up)) = calls.join_next() => {
                if let Some(up) = up {
                    sink.send(encode(&up)?).await?;
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() > SILENCE_LIMIT {
                    bail!("no word from the relay for {}s", SILENCE_LIMIT.as_secs());
                }
                sink.send(Message::Ping(Bytes::new())).await?;
            }
        }
    }
}

/// Run one call and return its reply. Waiting for a slot counts toward
/// [`CALL_TIMEOUT`], so the reply still reaches the relay in time.
async fn answer(engine: Engine, slots: Arc<Semaphore>, call: Call) -> Up {
    let Call { id, tool, args } = call;
    let run = async {
        let _slot = slots.acquire().await?;
        let tool: Tool = tool.parse()?;
        tools::call(&engine, tool, args).await
    };
    let result = match tokio::time::timeout(CALL_TIMEOUT, run).await {
        Ok(result) => result.map_err(|e| format!("{e:#}")),
        Err(_) => Err("the Mac didn't answer in time".into()),
    };
    Up::Reply { id, result }
}

/// The notice for `alert`, if `notify` allows one now and it's still news.
async fn tell(engine: Engine, alert: Alert, notify: NotifyWhen) -> Option<Up> {
    if notify == NotifyWhen::Away && !away() {
        return None;
    }
    match tokio::time::timeout(NOTICE_TIMEOUT, notice(&engine, &alert)).await {
        Ok(Ok(notice)) => notice.map(Up::Notify),
        Ok(Err(e)) => {
            eprintln!("relay: notification for {}: {e:#}", alert.chat);
            None
        }
        Err(_) => {
            eprintln!(
                "relay: notification for {}: the chat took too long to read",
                alert.chat
            );
            None
        }
    }
}

/// Build the notice for `alert` from the chat as it is now: its last
/// message, or the question it asks. `None` if the question is answered
/// already.
async fn notice(engine: &Engine, alert: &Alert) -> Result<Option<Notice>> {
    let view = engine.view(&alert.chat, 1).await?;
    let last = view.page.messages.last();
    let (kind, body) = match alert.kind {
        AlertKind::Finished => (NoticeKind::Finished, last.map(text).unwrap_or_default()),
        AlertKind::Failed => {
            let error = last.and_then(error);
            (
                NoticeKind::Failed,
                error.unwrap_or_else(|| "The run failed.".into()),
            )
        }
        AlertKind::Interrupted => (NoticeKind::Failed, "The run stopped: pi exited.".into()),
        AlertKind::Input => {
            let Some(question) = view.question else {
                return Ok(None);
            };
            let asked = question.message.filter(|_| question.title.is_empty());
            let asked = asked.unwrap_or(question.title);
            (NoticeKind::Input, format!("Needs your answer: {asked}"))
        }
    };
    let title = view.chat.title.unwrap_or_else(|| "Untitled chat".into());
    Ok(Some(Notice {
        kind,
        chat: view.chat.id,
        title: one_line(&title, BODY_CHARS).unwrap_or_default(),
        body: one_line(&body, BODY_CHARS).unwrap_or_default(),
    }))
}

/// A message's text, without its tool calls.
fn text(message: &transcript::Message) -> String {
    let parts = message.parts.iter().filter_map(|part| match part {
        Part::Text { text } => Some(text.as_str()),
        _ => None,
    });
    parts.collect::<Vec<_>>().join("\n")
}

fn error(message: &transcript::Message) -> Option<String> {
    message.parts.iter().find_map(|part| match part {
        Part::Error { error } => Some(error.clone()),
        _ => None,
    })
}

/// Whether nobody has touched the Mac's keyboard or mouse for [`AWAY_AFTER`].
/// If idle time can't be read, assume away: a missed notice is worse.
fn away() -> bool {
    // `kCGAnyInputEventType` (`~0`), a C macro the bindings leave out.
    let any_input = CGEventType(u32::MAX);
    let idle = CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::HIDSystemState,
        any_input,
    );
    Duration::try_from_secs_f64(idle)
        .ok()
        .is_none_or(|idle| idle >= AWAY_AFTER)
}

async fn dial(url: &str, token: &str) -> Result<Socket> {
    let mut request = url.into_client_request()?;
    let bearer = HeaderValue::from_str(&format!("Bearer {token}")).context("a bad token")?;
    request.headers_mut().insert(header::AUTHORIZATION, bearer);
    let (socket, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(socket)
}

fn hello() -> Up {
    Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

/// The relay's WebSocket URL for an `https://`, or local `http://`, address.
fn socket_url(relay: &str) -> Result<String> {
    let base = relay.trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        bail!("the relay address must start with https:// (got {relay})")
    };
    Ok(format!("{rest}{CONNECT_PATH}"))
}

/// Whether the relay refused the token.
fn rejected(error: &anyhow::Error) -> bool {
    use tokio_tungstenite::tungstenite::Error;
    matches!(
        error.downcast_ref::<Error>(),
        Some(Error::Http(response)) if response.status() == StatusCode::UNAUTHORIZED
    )
}

fn encode(up: &Up) -> Result<Message> {
    Ok(Message::Text(serde_json::to_string(up)?.into()))
}
