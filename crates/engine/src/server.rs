//! The engine's Unix socket: JSON lines carrying tool calls and their
//! answers, and pushed notices that the chat list changed.
//!
//! A call is `{"id": …, "call": "list_chats", "args": {…}}`, answered with
//! `{"id": …, "ok": …}` or `{"id": …, "error": "…"}`. Calls on one connection
//! run at the same time, so their answers can come in any order.
//! `{"id": …, "subscribe": "chats"}` is answered at once, and then pushes
//! `{"event": "chats", "revision": n}` after changes. Changes that come
//! faster than the client reads merge into one notice.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::engine::Engine;
use crate::tools::{self, Tool};

/// Records waiting to be written to one client.
const OUT_QUEUE: usize = 64;

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
            }) => {
                calls.spawn(call(engine.clone(), id, name, args, out.clone()));
                continue;
            }
            Ok(Request {
                id,
                call: None,
                subscribe: Some(topic),
                ..
            }) if topic == "chats" => {
                subscriptions.spawn(push_chats(engine.clone(), out.clone()));
                json!({"id": id, "ok": null})
            }
            Ok(Request {
                id,
                call: None,
                subscribe: Some(topic),
                ..
            }) => json!({"id": id, "error": format!("no subscription {topic}")}),
            Ok(Request { id, .. }) => {
                json!({"id": id, "error": "a request needs one of call or subscribe"})
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

/// Tell the client whenever the chat list changes, until it leaves.
async fn push_chats(engine: Engine, out: mpsc::Sender<Value>) {
    let mut changes = engine.subscribe();
    changes.mark_unchanged();
    while changes.changed().await.is_ok() {
        let revision = *changes.borrow_and_update();
        if out
            .send(json!({"event": "chats", "revision": revision}))
            .await
            .is_err()
        {
            break;
        }
    }
}
