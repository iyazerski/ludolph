use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use ludolph_relay_proto::{
    CONNECT_PATH, Down, PAIR_PATH, PING_EVERY, PROTOCOL, PairStart, Paired, Sealed, Up, pair_url,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode, Uri, header};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::engine::{Alert, AlertKind, Engine};
use crate::feed::{self, Feed};
use crate::notify::Changes;
use crate::power;
use crate::seal::{self, Keys, Opened, Purpose, Replays};
use crate::settings::Notify;
use crate::text::one_line;
use crate::tools::{self, Tool};
use crate::transcript::{self, Part, Window};

/// The relay answers every ping, so this is two missed pongs.
const SILENCE_LIMIT: Duration = Duration::from_secs(30);
/// After a network change, such as a VPN coming up, the old link is often
/// open but dead, so it gets a quick ping that must be answered.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// configd posts this when interfaces, addresses or routes change.
const NETWORK_CHANGE: &std::ffi::CStr = c"com.apple.system.config.network_change";
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Back off across short-lived connections too, such as protocol refusals after hello.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// Shorter than the relay's 30 s deadline; a timed-out call may still have executed.
const CALL_TIMEOUT: Duration = Duration::from_secs(25);
const OP_KEPT: Duration = Duration::from_secs(10 * 60);
const MAX_OPS: usize = 256;
const MAX_CALLS: usize = 16;
const MAX_QUEUED_CALLS: usize = 64;
const MAX_WATCHES: usize = 32;
const PHONE_FRAME: Duration = Duration::from_millis(200);
const FRAME_QUEUE: usize = 16;
const AWAY_AFTER: Duration = Duration::from_secs(120);
const NOTICE_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_CHARS: usize = 240;
const FORGET_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE: usize = 24 << 20;

pub const DEFAULT_RELAY: &str = "https://ludolph.iyazerski.dev";

/// Namespace Keychain accounts by engine folder to isolate development engines.
const KEYCHAIN_SERVICE: &str = "dev.iyazerski.ludolph.relay";
const NOT_FOUND: i32 = -25300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStore {
    Keychain,
    /// Tests use the private state file instead of the user's Keychain.
    File,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Link {
    pub relay: String,
    pub token: String,
    pub phones: Vec<String>,
}

// By hand, so logs never show a secret.
impl fmt::Debug for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Link")
            .field("relay", &self.relay)
            .field("token", &"<redacted>")
            .field("phones", &self.phones.len())
            .finish()
    }
}

pub struct Phones {
    linked: Mutex<HashMap<String, Arc<LinkedPhone>>>,
    replays_dir: PathBuf,
}

// By hand, so logs never show a key.
impl fmt::Debug for Phones {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Phones")
            .field("count", &self.lock().len())
            .finish()
    }
}

struct LinkedPhone {
    keys: Keys,
    replays: Mutex<Replays>,
    replays_path: PathBuf,
    ops: Ops,
    revoked: watch::Sender<bool>,
}

impl LinkedPhone {
    fn new(keys: Keys, replays_dir: &Path) -> Result<Self> {
        let replays_path = replays_dir.join(format!("{}.json", keys.phone));
        let replays = ludolph_json_file::load(&replays_path)?.unwrap_or_default();
        Ok(Self {
            keys,
            replays: Mutex::new(replays),
            replays_path,
            ops: Ops::default(),
            revoked: watch::channel(false).0,
        })
    }
}

type Outcome = Result<Value, String>;

type Op = (Instant, watch::Receiver<Option<Outcome>>);

/// Keep operations alive across relay timeouts so retrying an id cannot repeat its mutation.
#[derive(Default)]
struct Ops(Mutex<HashMap<String, Op>>);

impl Ops {
    async fn run(
        &self,
        id: String,
        wait: Duration,
        run: impl Future<Output = Outcome> + Send + 'static,
    ) -> Option<Outcome> {
        let mut done = {
            let mut ops = self.0.lock().expect("the ops lock isn't poisoned");
            let now = Instant::now();
            ops.retain(|_, (at, _)| now.duration_since(*at) < OP_KEPT);
            if let Some((_, done)) = ops.get(&id) {
                done.clone()
            } else {
                if ops.len() >= MAX_OPS
                    && let Some(oldest) = ops
                        .iter()
                        .min_by_key(|(_, (at, _))| *at)
                        .map(|(k, _)| k.clone())
                {
                    ops.remove(&oldest);
                }
                let (tx, done) = watch::channel(None);
                tokio::spawn(async move { tx.send_replace(Some(run.await)) });
                ops.insert(id, (now, done.clone()));
                done
            }
        };
        let ended = tokio::time::timeout(wait, done.wait_for(Option::is_some)).await;
        match ended {
            Ok(Ok(outcome)) => outcome.clone(),
            Ok(Err(_)) => Some(Err("the Mac dropped this call".into())),
            Err(_) => None,
        }
    }
}

impl Phones {
    /// Persist accepted nonces before dispatch, including across engine restarts.
    pub fn new(replays_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&replays_dir)
            .with_context(|| format!("creating {}", replays_dir.display()))?;
        Ok(Self {
            linked: Mutex::default(),
            replays_dir,
        })
    }

    pub fn set(&self, secrets: &[String]) -> Result<()> {
        let phones = secrets
            .iter()
            .map(|secret| {
                let phone = LinkedPhone::new(Keys::derive(secret)?, &self.replays_dir)?;
                Ok((phone.keys.phone.clone(), Arc::new(phone)))
            })
            .collect::<Result<_>>()?;
        *self.lock() = phones;
        Ok(())
    }

    pub fn add(&self, secret: &str) -> Result<()> {
        let phone = LinkedPhone::new(Keys::derive(secret)?, &self.replays_dir)?;
        self.lock()
            .insert(phone.keys.phone.clone(), Arc::new(phone));
        Ok(())
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.lock().keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn remove(&self, id: &str) {
        if let Some(phone) = self.lock().remove(id) {
            phone.revoked.send_replace(true);
        }
    }

    async fn open(&self, sealed: Sealed, purpose: Purpose) -> Result<(Arc<LinkedPhone>, Opened)> {
        let phone = self.lock().get(&sealed.phone).cloned().context(
            "this phone isn't linked to this Mac: link it again from the Mac's Settings",
        )?;
        tokio::task::spawn_blocking(move || {
            let opened = phone.keys.open(purpose, &sealed.data)?;
            {
                let mut replays = phone
                    .replays
                    .lock()
                    .expect("the replays lock isn't poisoned");
                replays.check(&opened, seal::now_ms())?;
                ludolph_json_file::save(&phone.replays_path, &*replays)?;
            }
            Ok((phone, opened))
        })
        .await?
    }

    fn all(&self) -> Vec<Arc<LinkedPhone>> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<LinkedPhone>>> {
        self.linked.lock().expect("the phones lock isn't poisoned")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phone {
    pub relay: String,
    pub status: LinkStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Pairing {
    Starting,
    #[serde(rename_all = "camelCase")]
    Waiting {
        url: String,
        expires_at: i64,
    },
    Linked,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Connecting,
    Connected,
    Rejected,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    relay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secrets: Option<Secrets>,
}

#[derive(Serialize, Deserialize)]
struct Secrets {
    token: String,
    phones: Vec<String>,
}

impl Link {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("relay.json")
    }

    pub fn load(state_dir: &Path, store: SecretStore) -> Result<Option<Self>> {
        let Some(saved) = ludolph_json_file::load::<Saved>(&Self::path(state_dir))? else {
            return Ok(None);
        };
        let secrets = match store {
            SecretStore::File => saved.secrets,
            SecretStore::Keychain => {
                match get_generic_password(KEYCHAIN_SERVICE, &keychain_account(state_dir)) {
                    Ok(bytes) => Some(
                        serde_json::from_slice(&bytes)
                            .context("the keychain's relay item isn't Ludolph's")?,
                    ),
                    Err(e) if e.code() == NOT_FOUND => None,
                    Err(e) => bail!("reading the relay secrets from the keychain: {e}"),
                }
            }
        };
        Ok(secrets.map(|secrets| Self {
            relay: saved.relay,
            token: secrets.token,
            phones: secrets.phones,
        }))
    }

    pub fn save(&self, state_dir: &Path, store: SecretStore) -> Result<()> {
        let secrets = Secrets {
            token: self.token.clone(),
            phones: self.phones.clone(),
        };
        let secrets = match store {
            SecretStore::File => Some(secrets),
            SecretStore::Keychain => {
                let account = keychain_account(state_dir);
                set_generic_password(KEYCHAIN_SERVICE, &account, &serde_json::to_vec(&secrets)?)
                    .map_err(|e| anyhow!("saving the relay secrets in the keychain: {e}"))?;
                None
            }
        };
        let saved = Saved {
            relay: self.relay.clone(),
            secrets,
        };
        ludolph_json_file::save(&Self::path(state_dir), &saved)
    }

    pub fn remove(state_dir: &Path, store: SecretStore) -> Result<()> {
        let path = Self::path(state_dir);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("removing {}", path.display()));
            }
            _ => {}
        }
        if store == SecretStore::Keychain {
            match delete_generic_password(KEYCHAIN_SERVICE, &keychain_account(state_dir)) {
                Err(e) if e.code() != NOT_FOUND => {
                    bail!("removing the relay secrets from the keychain: {e}")
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn keychain_account(state_dir: &Path) -> String {
    state_dir.display().to_string()
}

/// Reconnect until the relay rejects the token; retrying a rejected token cannot help.
pub(crate) async fn run(
    engine: Engine,
    link: Link,
    phones: Arc<Phones>,
    phone: watch::Sender<Option<Phone>>,
) {
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
    let mut power = power::changes()
        .inspect_err(|e| eprintln!("relay: the battery won't update: {e:#}"))
        .ok();
    let mut network = Changes::new(NETWORK_CHANGE)
        .inspect_err(|e| eprintln!("relay: network changes won't be noticed: {e:#}"))
        .ok();
    let mut backoff = MIN_BACKOFF;
    loop {
        let mut connected = None;
        let result = async {
            let socket = dial(&url, Some(&link.token)).await?;
            eprintln!("relay: connected to {url}");
            report(LinkStatus::Connected, None);
            connected = Some(Instant::now());
            serve(socket, &engine, &phones, &slots, &mut power, &mut network).await
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

pub async fn pair(
    relay: &str,
    token: Option<&str>,
    secret: &str,
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
                Paired::Code { code, expires_in } => {
                    shown(pair_url(relay, &code, secret), expires_in);
                }
                Paired::Linked { token } => return Ok(token),
            },
            Message::Close(Some(frame)) if !frame.reason.is_empty() => bail!("{}", frame.reason),
            Message::Close(_) => break,
            _ => {}
        }
    }
    bail!("the relay closed the link before a phone linked")
}

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

fn mac_name() -> String {
    let name = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok());
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

async fn serve(
    socket: Socket,
    engine: &Engine,
    phones: &Arc<Phones>,
    slots: &Arc<Semaphore>,
    power: &mut Option<Changes>,
    network: &mut Option<Changes>,
) -> Result<()> {
    let mut alerts = engine.alerts();
    let (mut sink, mut stream) = socket.split();
    sink.send(encode(&hello())?).await?;
    let mut battery = power::battery();
    sink.send(encode(&Up::Power { battery })?).await?;
    let mut calls: JoinSet<Option<Up>> = JoinSet::new();
    let mut watches: JoinSet<()> = JoinSet::new();
    let mut watching: HashMap<u64, AbortHandle> = HashMap::new();
    let (frames_tx, mut frames) = mpsc::channel::<Up>(FRAME_QUEUE);
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut deadline = Instant::now() + SILENCE_LIMIT;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => bail!("the relay stopped answering"),
            frame = stream.next() => {
                deadline = Instant::now() + SILENCE_LIMIT;
                match frame {
                    Some(Ok(Message::Text(body))) => match serde_json::from_str::<Down>(&body) {
                        Ok(Down::Call { id, .. }) if calls.len() >= MAX_QUEUED_CALLS => {
                            let busy = Up::Reply { id, result: Err("this Mac is busy; try again".into()) };
                            sink.send(encode(&busy)?).await?;
                        }
                        Ok(Down::Call { id, sealed }) => {
                            let reply = match phones.open(sealed, Purpose::Call).await {
                                Ok((phone, call)) => {
                                    let reply = answer(engine.clone(), slots.clone(), phone, call);
                                    calls.spawn(async move { Some(Up::Reply { id, result: reply.await }) });
                                    continue;
                                }
                                Err(e) => Up::Reply { id, result: Err(format!("{e:#}")) },
                            };
                            sink.send(encode(&reply)?).await?;
                        }
                        Ok(Down::Watch { id, sealed }) => {
                            while watches.try_join_next().is_some() {}
                            watching.retain(|_, handle| !handle.is_finished());
                            let opened = if watching.len() >= MAX_WATCHES {
                                Err(anyhow!("this Mac follows too many chats; close some"))
                            } else {
                                phones.open(sealed, Purpose::Watch).await
                            };
                            let (phone, request) = match opened {
                                Ok(opened) => opened,
                                Err(e) => {
                                    let refused = Up::Frame { id, frame: Err(format!("{e:#}")) };
                                    sink.send(encode(&refused)?).await?;
                                    continue;
                                }
                            };
                            let follow = watch(engine.clone(), id, phone, request, frames_tx.clone());
                            let handle = watches.spawn(follow);
                            if let Some(old) = watching.insert(id, handle) {
                                old.abort();
                            }
                        }
                        Ok(Down::Unwatch { id }) => {
                            if let Some(handle) = watching.remove(&id) {
                                handle.abort();
                            }
                        }
                        Err(e) => eprintln!("relay: skipped a bad message: {e}"),
                    },
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            alert = alerts.recv() => match alert {
                Ok(alert) => _ = calls.spawn(tell(engine.clone(), phones.clone(), alert)),
                Err(RecvError::Lagged(missed)) => eprintln!("relay: skipped {missed} notifications"),
                Err(RecvError::Closed) => bail!("the engine stopped"),
            },
            changed = change(network) => {
                if let Err(e) = changed {
                    eprintln!("relay: network changes won't be noticed: {e:#}");
                    *network = None;
                }
                sink.send(Message::Ping(Bytes::new())).await?;
                deadline = deadline.min(Instant::now() + PROBE_TIMEOUT);
            }
            changed = change(power) => {
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
            _ = ping.tick() => sink.send(Message::Ping(Bytes::new())).await?,
        }
    }
}

#[derive(Deserialize)]
struct Call {
    tool: String,
    #[serde(default)]
    args: Value,
    op: Option<String>,
}

#[derive(Deserialize)]
struct WatchRequest {
    chat: String,
}

/// Include slot queueing in CALL_TIMEOUT so even a busy response reaches the relay in time.
async fn answer(
    engine: Engine,
    slots: Arc<Semaphore>,
    phone: Arc<LinkedPhone>,
    call: Opened,
) -> Result<String, String> {
    let reply = match serde_json::from_str::<Call>(&call.text) {
        Ok(Call { tool, args, op }) => {
            let run = async move {
                let _slot = slots.acquire_owned().await.map_err(|e| e.to_string())?;
                let parsed: Tool = tool.parse().map_err(|e| format!("{e:#}"))?;
                if !parsed.for_phone() {
                    return Err(format!("the phone can't call {tool}"));
                }
                tools::call(&engine, parsed, args)
                    .await
                    .map_err(|e| format!("{e:#}"))
            };
            let tagged = op.is_some();
            let outcome = match op {
                Some(op) => phone.ops.run(op, CALL_TIMEOUT, run).await,
                None => tokio::time::timeout(CALL_TIMEOUT, run).await.ok(),
            };
            match outcome {
                Some(Ok(value)) => json!({ "ok": value }),
                Some(Err(e)) => json!({ "error": e }),
                None if tagged => json!({ "pending": true }),
                None => json!({ "error": "the Mac didn't answer in time" }),
            }
        }
        Err(e) => json!({ "error": format!("a bad call: {e}") }),
    };
    let sealed = phone
        .keys
        .seal(Purpose::Reply(call.nonce), &reply.to_string());
    sealed.map(|(data, _)| data).map_err(|e| format!("{e:#}"))
}

async fn watch(
    engine: Engine,
    id: u64,
    phone: Arc<LinkedPhone>,
    request: Opened,
    out: mpsc::Sender<Up>,
) {
    let mut revoked = phone.revoked.subscribe();
    let ended = out.clone();
    let watch_nonce = request.nonce;
    let mut sent = 0;
    let mut send = move |frame: Value| {
        let n = sent;
        sent += 1;
        let frame = phone
            .keys
            .seal(Purpose::Frame(watch_nonce, n), &frame.to_string())
            .map(|(data, _)| data)
            .map_err(|e| format!("{e:#}"));
        let out = out.clone();
        async move { out.send(Up::Frame { id, frame }).await.is_ok() }
    };
    let chat = match serde_json::from_str::<WatchRequest>(&request.text) {
        Ok(request) => request.chat,
        Err(e) => {
            send(json!({"error": format!("a bad request: {e}")})).await;
            return;
        }
    };
    match engine.subscribe_chat(&chat) {
        Ok(changes) => {
            let feed = Feed::new(engine, chat, false);
            let unlinked = tokio::select! {
                biased;
                _ = revoked.wait_for(|revoked| *revoked) => true,
                _ = feed::follow(changes, feed, PHONE_FRAME, send) => false,
            };
            if unlinked {
                let _ = ended
                    .send(Up::Frame {
                        id,
                        frame: Err("this phone was unlinked from this Mac".to_owned()),
                    })
                    .await;
            }
        }
        Err(e) => _ = send(json!({"error": format!("{e:#}")})).await,
    }
}

async fn change(changes: &mut Option<Changes>) -> Result<()> {
    match changes {
        Some(changes) => changes.next().await,
        None => std::future::pending().await,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Notice {
    kind: NoticeKind,
    chat: String,
    title: String,
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum NoticeKind {
    Finished,
    Input,
    Failed,
    Interrupted,
}

async fn tell(engine: Engine, phones: Arc<Phones>, alert: Alert) -> Option<Up> {
    if engine.settings().notify == Notify::Away && !away() {
        return None;
    }
    let sealed = |notice: Notice| {
        let text = serde_json::to_string(&notice)?;
        let seal = |phone: Arc<LinkedPhone>| {
            let (data, _) = phone.keys.seal(Purpose::Notice, &text)?;
            let phone = phone.keys.phone.clone();
            Ok(Sealed { phone, data })
        };
        phones.all().into_iter().map(seal).collect::<Result<_>>()
    };
    match tokio::time::timeout(NOTICE_TIMEOUT, notice(&engine, &alert)).await {
        Ok(Ok(notice)) => match notice.map(sealed).transpose() {
            Ok(notices) => notices.map(|notices| Up::Notify { notices }),
            Err(e) => {
                eprintln!("relay: notification for {}: {e:#}", alert.chat);
                None
            }
        },
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

/// Assume away if input idle time is unavailable; prefer an extra notice to a missed one.
fn away() -> bool {
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
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let (socket, _) =
        tokio_tungstenite::connect_async_with_config(request, Some(config), false).await?;
    Ok(socket)
}

fn hello() -> Up {
    Up::Hello {
        protocol: PROTOCOL,
        version: crate::VERSION.into(),
    }
}

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

/// Permit plaintext tokens only on loopback.
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
