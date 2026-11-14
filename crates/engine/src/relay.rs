//! The link to the relay, which brings the phone's calls to this Mac
//! (`docs/design.md`, "Phone and relay").
//!
//! The engine keeps one outgoing WebSocket to the relay, and answers its
//! calls with the same tools the app uses. Only those tools exist, so the
//! relay can't reach anything else on the Mac. It streams the chats the phone
//! has open as their feeds' frames. It also passes the chats'
//! alerts on, so the phone can notify you, and the battery, which the
//! phone shows.
//!
//! A phone links by scanning a QR code from Settings: [`pair`] waits on the
//! relay for a phone to claim its code, and gets this Mac's token if the
//! relay didn't know it yet.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use ludolph_relay_proto::{
    CONNECT_PATH, Down, Notice, NoticeKind, PAIR_PATH, PROTOCOL, PairStart, Paired, Up, pair_url,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode, Uri, header};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::engine::{Alert, AlertKind, Engine};
use crate::feed::{self, Feed};
use crate::power;
use crate::settings::Notify;
use crate::text::one_line;
use crate::tools::{self, Tool};
use crate::transcript::{self, Part, Window};

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
/// Chats followed at once on one connection; more are refused.
const MAX_WATCHES: usize = 32;
/// The shortest time between two frames of a chat the phone follows: slower
/// than on the Mac, since each goes over the network.
const PHONE_FRAME: Duration = Duration::from_millis(200);
/// Frames waiting to be sent; a followed chat waits when they're full.
const FRAME_QUEUE: usize = 16;
/// The Mac counts as in use if it had keyboard or mouse input this recently.
const AWAY_AFTER: Duration = Duration::from_secs(120);
/// How long building one notice (reading its chat) may take.
const NOTICE_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_CHARS: usize = 240;
/// How long the relay has to confirm an unlink.
const FORGET_TIMEOUT: Duration = Duration::from_secs(10);

/// The relay a phone links through unless Settings names another.
pub const DEFAULT_RELAY: &str = "https://ludolph.iyazerski.dev";

/// The Keychain item's service; its account is the engine's folder, so a
/// throwaway engine can't change the installed one's token.
const KEYCHAIN_SERVICE: &str = "dev.iyazerski.ludolph.relay";
/// `errSecItemNotFound`.
const NOT_FOUND: i32 = -25300;

/// Where this Mac's relay token is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenStore {
    /// The login keychain, beside `relay.json` with the relay's address.
    Keychain,
    /// In `relay.json` itself, readable only by you, for tests that mustn't
    /// touch the Mac's keychain.
    File,
}

/// The relay's address and this Mac's token, from linking a phone.
#[derive(Clone, PartialEq, Eq)]
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

/// How this Mac's link to the relay stands, as Settings shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phone {
    pub relay: String,
    pub status: LinkStatus,
    /// Why the last try to connect failed, while it tries again.
    pub error: Option<String>,
}

/// A phone linking through this Mac, as Settings shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Pairing {
    /// Getting a code from the relay.
    Starting,
    /// Waiting for a phone to open `url`, until `expires_at` (Unix ms).
    #[serde(rename_all = "camelCase")]
    Waiting {
        url: String,
        expires_at: i64,
    },
    /// A phone linked.
    Linked,
    Failed {
        error: String,
    },
}

/// Whether the engine is connected to the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Connecting,
    Connected,
    /// The relay refused the token, so a phone must link again.
    Rejected,
}

/// `relay.json`: the relay's address, and the token when it isn't in the
/// keychain.
#[derive(Serialize, Deserialize)]
struct Saved {
    relay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

impl Link {
    /// Where the link is kept in the engine's folder.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("relay.json")
    }

    /// The saved link, if this Mac has one with its token.
    ///
    /// # Errors
    ///
    /// Fails if the file or the keychain can't be read.
    pub fn load(state_dir: &Path, store: TokenStore) -> Result<Option<Self>> {
        let Some(saved) = ludolph_json_file::load::<Saved>(&Self::path(state_dir))? else {
            return Ok(None);
        };
        let token = match store {
            TokenStore::File => saved.token,
            TokenStore::Keychain => {
                match get_generic_password(KEYCHAIN_SERVICE, &keychain_account(state_dir)) {
                    Ok(bytes) => {
                        Some(String::from_utf8(bytes).context("the relay token isn't text")?)
                    }
                    Err(e) if e.code() == NOT_FOUND => None,
                    Err(e) => bail!("reading the relay token from the keychain: {e}"),
                }
            }
        };
        Ok(token.map(|token| Self {
            relay: saved.relay,
            token,
        }))
    }

    /// Save the link, with the token where `store` keeps it.
    ///
    /// # Errors
    ///
    /// Fails if the file or the keychain can't be written.
    pub fn save(&self, state_dir: &Path, store: TokenStore) -> Result<()> {
        let token = match store {
            TokenStore::File => Some(self.token.clone()),
            TokenStore::Keychain => {
                let account = keychain_account(state_dir);
                set_generic_password(KEYCHAIN_SERVICE, &account, self.token.as_bytes())
                    .map_err(|e| anyhow!("saving the relay token in the keychain: {e}"))?;
                None
            }
        };
        let saved = Saved {
            relay: self.relay.clone(),
            token,
        };
        ludolph_json_file::save(&Self::path(state_dir), &saved)
    }

    /// Forget the saved link and its token.
    ///
    /// # Errors
    ///
    /// Fails if the file or the keychain item can't be removed.
    pub fn remove(state_dir: &Path, store: TokenStore) -> Result<()> {
        let path = Self::path(state_dir);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("removing {}", path.display()));
            }
            _ => {}
        }
        if store == TokenStore::Keychain {
            match delete_generic_password(KEYCHAIN_SERVICE, &keychain_account(state_dir)) {
                Err(e) if e.code() != NOT_FOUND => {
                    bail!("removing the relay token from the keychain: {e}")
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The keychain account of the engine whose folder is `state_dir`.
fn keychain_account(state_dir: &Path) -> String {
    state_dir.display().to_string()
}

/// Stay connected to the relay, answer its calls, and pass alerts on as
/// the settings allow, reconnecting when the link drops, and keep `phone`
/// up to date. It returns only when the relay rejects the token, since
/// trying again can't help then.
pub(crate) async fn run(engine: Engine, link: Link, phone: watch::Sender<Option<Phone>>) {
    let report = |status, error: Option<String>| {
        phone.send_replace(Some(Phone {
            relay: link.relay.clone(),
            status,
            error,
        }));
    };
    let url = match socket_url(&link.relay, CONNECT_PATH) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("relay: {e:#}");
            report(LinkStatus::Connecting, Some(e.root_cause().to_string()));
            return;
        }
    };
    let slots = Arc::new(Semaphore::new(MAX_CALLS));
    let mut power = power::Changes::new()
        .inspect_err(|e| eprintln!("relay: the battery won't update: {e:#}"))
        .ok();
    let mut backoff = MIN_BACKOFF;
    loop {
        let mut connected = None;
        let result = async {
            let socket = dial(&url, Some(&link.token)).await?;
            eprintln!("relay: connected to {url}");
            report(LinkStatus::Connected, None);
            connected = Some(Instant::now());
            serve(socket, &engine, &slots, &mut power).await
        }
        .await;
        if connected.is_some_and(|at| at.elapsed() >= STABLE_AFTER) {
            backoff = MIN_BACKOFF;
        }
        match result {
            Ok(()) => {
                eprintln!("relay: the relay closed the link; reconnecting");
                report(LinkStatus::Connecting, None);
            }
            Err(e) if is_rejected(&e) => {
                eprintln!("relay: the relay rejected this Mac's token; link a phone again");
                report(LinkStatus::Rejected, None);
                return;
            }
            Err(e) => {
                eprintln!("relay: {e:#}");
                report(LinkStatus::Connecting, Some(e.root_cause().to_string()));
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Wait on `relay` for a phone to link through this Mac, calling `shown`
/// with the page the QR code opens and the seconds it works for. `token`
/// is this Mac's, if the relay knows it. Returns the token the relay gives
/// a Mac it didn't know.
///
/// # Errors
///
/// Fails if the relay can't be reached, refuses `token` ([`is_rejected`]),
/// or closes before a phone links, as when the code expires.
pub async fn pair(
    relay: &str,
    token: Option<&str>,
    mut shown: impl FnMut(String, u64),
) -> Result<Option<String>> {
    let mut socket = dial(&socket_url(relay, PAIR_PATH)?, token).await?;
    let start = PairStart {
        protocol: PROTOCOL,
        name: mac_name(),
    };
    socket
        .send(Message::Text(serde_json::to_string(&start)?.into()))
        .await?;
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(body) => match serde_json::from_str(&body)? {
                Paired::Code { code, expires_in } => shown(pair_url(relay, &code), expires_in),
                Paired::Linked { token } => return Ok(token),
            },
            Message::Close(Some(frame)) if !frame.reason.is_empty() => bail!("{}", frame.reason),
            Message::Close(_) => break,
            _ => {}
        }
    }
    bail!("the relay closed the link before a phone linked")
}

/// Tell the relay to forget this Mac and its token.
///
/// # Errors
///
/// Fails if the relay can't be reached or doesn't confirm in time.
pub async fn forget(link: &Link) -> Result<()> {
    let url = socket_url(&link.relay, CONNECT_PATH)?;
    let confirmed = async {
        let mut socket = dial(&url, Some(&link.token)).await?;
        socket.send(encode(&hello())?).await?;
        socket.send(encode(&Up::Forget)?).await?;
        // The relay closes once it has forgotten the Mac.
        while let Some(message) = socket.next().await {
            if message?.is_close() {
                break;
            }
        }
        anyhow::Ok(())
    };
    tokio::time::timeout(FORGET_TIMEOUT, confirmed)
        .await
        .context("the relay didn't answer")?
}

/// The Mac's name, as Sharing settings has it, which the phone shows.
fn mac_name() -> String {
    let name = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok());
    // The relay takes up to 60 characters.
    let name: String = name
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect();
    if name.is_empty() {
        "Mac".to_owned()
    } else {
        name
    }
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
    power: &mut Option<power::Changes>,
) -> Result<()> {
    let mut alerts = engine.alerts();
    let (mut sink, mut stream) = socket.split();
    sink.send(encode(&hello())?).await?;
    let mut battery = power::battery();
    sink.send(encode(&Up::Power { battery })?).await?;
    // Calls and notices being built. Dropping the set when the connection
    // ends stops them, since they have nowhere to go.
    let mut calls: JoinSet<Option<Up>> = JoinSet::new();
    // The chats the phone follows, by watch id. Their tasks end with the
    // connection, like the calls.
    let mut watches: JoinSet<()> = JoinSet::new();
    let mut watching: HashMap<u64, AbortHandle> = HashMap::new();
    let (frames_tx, mut frames) = mpsc::channel::<Up>(FRAME_QUEUE);
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            frame = stream.next() => {
                heard = Instant::now();
                match frame {
                    Some(Ok(Message::Text(body))) => match serde_json::from_str::<Down>(&body) {
                        Ok(Down::Call { id, .. }) if calls.len() >= MAX_QUEUED_CALLS => {
                            let busy = Up::Reply { id, result: Err("this Mac is busy; try again".into()) };
                            sink.send(encode(&busy)?).await?;
                        }
                        Ok(Down::Call { id, tool, args }) => {
                            let reply = answer(engine.clone(), slots.clone(), id, tool, args);
                            calls.spawn(async { Some(reply.await) });
                        }
                        Ok(Down::Watch { id, chat }) => {
                            while watches.try_join_next().is_some() {}
                            watching.retain(|_, handle| !handle.is_finished());
                            if watching.len() >= MAX_WATCHES {
                                let full = Up::Frame { id, frame: serde_json::json!({"error": "this Mac follows too many chats; close some"}) };
                                sink.send(encode(&full)?).await?;
                                continue;
                            }
                            let handle = watches.spawn(watch(engine.clone(), id, chat, frames_tx.clone()));
                            if let Some(old) = watching.insert(id, handle) {
                                old.abort();
                            }
                        }
                        Ok(Down::Unwatch { id }) => {
                            if let Some(handle) = watching.remove(&id) {
                                handle.abort();
                            }
                        }
                        // The relay times out the call it holds.
                        Err(e) => eprintln!("relay: skipped a bad message: {e}"),
                    },
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            alert = alerts.recv() => match alert {
                Ok(alert) => _ = calls.spawn(tell(engine.clone(), alert)),
                Err(RecvError::Lagged(missed)) => eprintln!("relay: skipped {missed} notifications"),
                Err(RecvError::Closed) => bail!("the engine stopped"),
            },
            changed = power_change(power) => {
                if let Err(e) = changed {
                    eprintln!("relay: the battery won't update: {e:#}");
                    *power = None;
                }
                let now = power::battery();
                if now != battery {
                    battery = now;
                    sink.send(encode(&Up::Power { battery })?).await?;
                }
            }
            Some(Ok(up)) = calls.join_next() => {
                if let Some(up) = up {
                    sink.send(encode(&up)?).await?;
                }
            }
            Some(frame) = frames.recv() => sink.send(encode(&frame)?).await?,
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
async fn answer(
    engine: Engine,
    slots: Arc<Semaphore>,
    id: u64,
    tool: String,
    args: serde_json::Value,
) -> Up {
    let run = async {
        let _slot = slots.acquire().await?;
        let parsed: Tool = tool.parse()?;
        if !parsed.for_phone() {
            bail!("the phone can't call {tool}");
        }
        tools::call(&engine, parsed, args).await
    };
    let result = match tokio::time::timeout(CALL_TIMEOUT, run).await {
        Ok(result) => result.map_err(|e| format!("{e:#}")),
        Err(_) => Err("the Mac didn't answer in time".into()),
    };
    Up::Reply { id, result }
}

/// Follow chat `chat` for the phone, sending its frames to `out` as watch
/// `id`'s until the relay stops it or the connection ends.
async fn watch(engine: Engine, id: u64, chat: String, out: mpsc::Sender<Up>) {
    let send = move |frame| {
        let out = out.clone();
        async move { out.send(Up::Frame { id, frame }).await.is_ok() }
    };
    match engine.subscribe_chat(&chat) {
        Ok(changes) => {
            let feed = Feed::new(engine, chat, false);
            feed::follow(changes, feed, PHONE_FRAME, send).await;
        }
        Err(e) => _ = send(serde_json::json!({"error": format!("{e:#}")})).await,
    }
}

/// Wait for the battery to change; forever without a way to tell.
async fn power_change(power: &mut Option<power::Changes>) -> Result<()> {
    match power {
        Some(changes) => changes.next().await,
        None => std::future::pending().await,
    }
}

/// The notice for `alert`, if the settings allow one now and it's still
/// news.
async fn tell(engine: Engine, alert: Alert) -> Option<Up> {
    if engine.settings().notify == Notify::Away && !away() {
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
    let view = engine.view(&alert.chat, Window::Latest(1)).await?;
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
        AlertKind::Interrupted => (
            NoticeKind::Interrupted,
            "The run stopped: pi exited.".into(),
        ),
        AlertKind::Input => {
            let Some(question) = view.question else {
                return Ok(None);
            };
            let asked = question.message.filter(|_| question.title.is_empty());
            let asked = asked.unwrap_or(question.title);
            (NoticeKind::Input, format!("Needs your answer: {asked}"))
        }
    };
    // Like the list, the first prompt stands in until there's a title.
    let first = view.chat.summary.and_then(|s| s.first_prompt);
    let title = view
        .chat
        .title
        .or(first)
        .unwrap_or_else(|| "Untitled chat".into());
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

async fn dial(url: &str, token: Option<&str>) -> Result<Socket> {
    let mut request = url.into_client_request()?;
    if let Some(token) = token {
        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).context("a bad token")?;
        request.headers_mut().insert(header::AUTHORIZATION, bearer);
    }
    let (socket, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(socket)
}

fn hello() -> Up {
    Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

/// The WebSocket URL of `path` on an `https://`, or local `http://`, relay.
///
/// # Errors
///
/// Fails for any other address.
pub fn socket_url(relay: &str, path: &str) -> Result<String> {
    let base = relay.trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://")
        && on_this_mac(base)
    {
        format!("ws://{rest}")
    } else {
        bail!("the relay address must start with https:// (got {relay})")
    };
    Ok(format!("{rest}{path}"))
}

/// Whether `address` points at this Mac, the only place the token can go
/// without TLS and not be overheard.
fn on_this_mac(address: &str) -> bool {
    let Some(host) = address
        .parse::<Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_owned))
    else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether the relay refused the token.
pub fn is_rejected(error: &anyhow::Error) -> bool {
    use tokio_tungstenite::tungstenite::Error;
    matches!(
        error.downcast_ref::<Error>(),
        Some(Error::Http(response)) if response.status() == StatusCode::UNAUTHORIZED
    )
}

fn encode(up: &Up) -> Result<Message> {
    Ok(Message::Text(serde_json::to_string(up)?.into()))
}
