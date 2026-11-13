//! The engine's Unix socket: JSON lines carrying tool calls and their
//! answers, and pushed notices that something changed.
//!
//! A call is `{"id": …, "call": "list_chats", "args": {…}}`, answered with
//! `{"id": …, "ok": …}` or `{"id": …, "error": "…"}`. Calls on one connection
//! run at the same time, so their answers can come in any order.
//!
//! A subscription is answered the same way, and then pushes notices:
//!
//! - `{"id": …, "subscribe": "chats"}` pushes `{"event": "chats",
//!   "revision": n}` when the chat list changes.
//! - `{"id": …, "subscribe": "chat", "args": {"chat": …}}` pushes
//!   `{"event": "chat", "chat": …, "revision": n}` when the chat's view does.
//!
//! Changes that come faster than the client reads merge into one notice.
//! `{"id": …, "unsubscribe": <the subscription's id>}` ends one.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinSet};

use crate::engine::Engine;
use crate::tools::{self, Tool};

/// Records waiting to be written to one client.
const OUT_QUEUE: usize = 64;

/// The socket in the engine's folder.
pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join("engine.sock")
}

/// Call `tool` on the engine listening at `path`, and return its answer.
/// The connection carries this one call, so the first line back answers it.
///
/// # Errors
///
/// Fails if no engine listens there, or the tool fails.
pub async fn request(path: &Path, tool: &str, args: Value) -> Result<Value> {
    let stream = UnixStream::connect(path).await.with_context(|| {
        format!(
            "no engine at {}: start it with ludolph install",
            path.display()
        )
    })?;
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_vec(&json!({"id": 1, "call": tool, "args": args}))?;
    line.push(b'\n');
    write.write_all(&line).await?;
    let mut answer = Vec::new();
    if BufReader::new(read).read_until(b'\n', &mut answer).await? == 0 {
        bail!("the engine closed the connection");
    }
    let mut answer: Value = serde_json::from_slice(&answer)?;
    if let Some(error) = answer["error"].as_str() {
        bail!("{tool}: {error}");
    }
    Ok(answer["ok"].take())
}

/// Listen on `path`, readable and writable only by the user.
///
/// # Errors
///
/// Fails if another engine is listening there, or the socket can't be made.
pub fn bind(path: &Path) -> Result<UnixListener> {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => bail!("an engine is already running at {}", path.display()),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        // A socket left by an engine that stopped.
        Err(_) => {
            std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?
        }
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("listening on {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Answer clients on `listener` until it fails.
///
/// # Errors
///
/// Fails if accepting a connection fails.
pub async fn serve(engine: Engine, listener: UnixListener) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await.context("accepting a client")?;
        tokio::spawn(connection(engine.clone(), stream));
    }
}

#[derive(Deserialize)]
struct Request {
    id: Value,
    call: Option<String>,
    #[serde(default)]
    args: Value,
    subscribe: Option<String>,
    unsubscribe: Option<Value>,
}

async fn connection(engine: Engine, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let (out, mut queue) = mpsc::channel::<Value>(OUT_QUEUE);
    let writer = tokio::spawn(async move {
        while let Some(record) = queue.recv().await {
            let mut line = serde_json::to_vec(&record).expect("JSON values serialize");
            line.push(b'\n');
            if write.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    let mut calls = JoinSet::new();
    let mut subscriptions = JoinSet::new();
    // By the subscribing request's id, in its JSON form.
    let mut handles: HashMap<String, AbortHandle> = HashMap::new();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let record = match serde_json::from_slice::<Request>(&line) {
            Ok(Request {
                id,
                call: Some(name),
                args,
                subscribe: None,
                unsubscribe: None,
            }) => {
                calls.spawn(call(engine.clone(), id, name, args, out.clone()));
                continue;
            }
            Ok(Request {
                id,
                call: None,
                subscribe: Some(topic),
                args,
                unsubscribe: None,
            }) => match subscription(&engine, &topic, args) {
                Ok((changes, event)) => {
                    let handle = subscriptions.spawn(push(changes, event, out.clone()));
                    if let Some(old) = handles.insert(id.to_string(), handle) {
                        old.abort();
                    }
                    json!({"id": id, "ok": null})
                }
                Err(e) => json!({"id": id, "error": format!("{e:#}")}),
            },
            Ok(Request {
                id,
                call: None,
                subscribe: None,
                unsubscribe: Some(subscription),
                ..
            }) => {
                if let Some(handle) = handles.remove(&subscription.to_string()) {
                    handle.abort();
                }
                json!({"id": id, "ok": null})
            }
            Ok(Request { id, .. }) => {
                json!({"id": id, "error": "a request needs one of call, subscribe or unsubscribe"})
            }
            Err(e) => json!({"id": null, "error": format!("invalid request: {e}")}),
        };
        if out.send(record).await.is_err() {
            break;
        }
    }
    // A client may close its side once it has sent everything, and still
    // read the answers.
    while calls.join_next().await.is_some() {}
    subscriptions.abort_all();
    drop(out);
    // Done once every answer is written, or the client left.
    let _ = writer.await;
}

/// Run one tool call and queue its answer.
async fn call(engine: Engine, id: Value, name: String, args: Value, out: mpsc::Sender<Value>) {
    let answer = match name.parse::<Tool>() {
        Ok(tool) => tools::call(&engine, tool, args).await,
        Err(e) => Err(e),
    };
    let record = match answer {
        Ok(value) => json!({"id": id, "ok": value}),
        Err(e) => json!({"id": id, "error": format!("{e:#}")}),
    };
    // A client that left doesn't need its answer.
    let _ = out.send(record).await;
}

/// The changes `topic` follows, and the notice that tells of them.
fn subscription(
    engine: &Engine,
    topic: &str,
    args: Value,
) -> Result<(watch::Receiver<u64>, Value)> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ChatArgs {
        chat: String,
    }
    match topic {
        "chats" => Ok((engine.subscribe(), json!({"event": "chats"}))),
        "chat" => {
            let args: ChatArgs = serde_json::from_value(args).context("invalid arguments")?;
            let changes = engine.subscribe_chat(&args.chat)?;
            Ok((changes, json!({"event": "chat", "chat": args.chat})))
        }
        _ => bail!("no subscription {topic}"),
    }
}

/// Send `event` with the latest revision whenever `changes` moves, until
/// the client leaves.
async fn push(mut changes: watch::Receiver<u64>, event: Value, out: mpsc::Sender<Value>) {
    changes.mark_unchanged();
    while changes.changed().await.is_ok() {
        let mut notice = event.clone();
        notice["revision"] = json!(*changes.borrow_and_update());
        if out.send(notice).await.is_err() {
            break;
        }
    }
}
