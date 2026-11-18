use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinSet};

use crate::engine::{AlertKind, Engine};
use crate::feed::{self, Feed};
use crate::tools::{self, Tool};

/// Throttle layouts to about seven frames per second while replies stream.
pub const FRAME: Duration = Duration::from_millis(150);

const OUT_QUEUE: usize = 64;

pub const MAX_LINE: u64 = 64 << 20;

pub fn bind(path: &Path) -> Result<UnixListener> {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => bail!("an engine is already running at {}", path.display()),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(_) => {
            std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?
        }
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("listening on {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

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
    let mut handles: HashMap<String, AbortHandle> = HashMap::new();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        // Reap finished requests on long-lived connections.
        while calls.try_join_next().is_some() {}
        while subscriptions.try_join_next().is_some() {}
        line.clear();
        match (&mut reader)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // An oversized line destroys request framing; close rather than keep reading.
        if !line.ends_with(b"\n") && line.len() as u64 > MAX_LINE {
            let refused =
                json!({"id": null, "error": format!("a request longer than {MAX_LINE} bytes")});
            let _ = out.send(refused).await;
            break;
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
            }) => match subscription(&engine, &topic, args, id.clone(), out.clone()) {
                Ok(pushing) => {
                    let handle = subscriptions.spawn(pushing);
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
    let _ = writer.await;
}

async fn call(engine: Engine, id: Value, name: String, args: Value, out: mpsc::Sender<Value>) {
    let answer = match name.parse::<Tool>() {
        Ok(tool) => tools::call(&engine, tool, args).await,
        Err(e) => Err(e),
    };
    let record = match answer {
        Ok(value) => json!({"id": id, "ok": value}),
        Err(e) => json!({"id": id, "error": format!("{e:#}")}),
    };
    let _ = out.send(record).await;
}

type Pushing = Pin<Box<dyn Future<Output = ()> + Send>>;

fn subscription(
    engine: &Engine,
    topic: &str,
    args: Value,
    id: Value,
    out: mpsc::Sender<Value>,
) -> Result<Pushing> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ChatArgs {
        chat: String,
        #[serde(default)]
        thinking: bool,
    }
    match topic {
        "models" => Ok(Box::pin(push(
            engine.subscribe_models(),
            json!({"event": "models", "subscription": id}),
            out,
        ))),
        "sign_in" => Ok(Box::pin(push(
            engine.sign_in().subscribe(),
            json!({"event": "sign_in", "subscription": id}),
            out,
        ))),
        "chats" => Ok(Box::pin(push(
            engine.subscribe(),
            json!({"event": "chats", "subscription": id}),
            out,
        ))),
        "job_notices" => {
            let mut alerts = engine.alerts();
            Ok(Box::pin(async move {
                loop {
                    match alerts.recv().await {
                        Ok(alert) => {
                            if let AlertKind::Job { id: job, title, status } = alert.kind
                                && out.send(json!({
                                    "event": "job_notices", "subscription": id,
                                    "chat": alert.chat, "id": job, "title": title, "status": status,
                                })).await.is_err()
                            {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }))
        }
        "chat" => {
            let args: ChatArgs = serde_json::from_value(args).context("invalid arguments")?;
            let changes = engine.subscribe_chat(&args.chat)?;
            let feed = Feed::new(engine.clone(), args.chat.clone(), args.thinking);
            let chat = args.chat;
            Ok(Box::pin(feed::follow(
                changes,
                feed,
                FRAME,
                move |mut frame| {
                    frame["event"] = json!("chat");
                    frame["chatId"] = json!(chat);
                    frame["subscription"] = id.clone();
                    let out = out.clone();
                    async move { out.send(frame).await.is_ok() }
                },
            )))
        }
        _ => bail!("no subscription {topic}"),
    }
}

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
