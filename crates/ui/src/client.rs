//! A connection to the engine's socket (`ludolph_engine::server`): tool
//! calls with their answers, and subscriptions that push what changes.
//!
//! The engine runs on its own, so it may start after the app, restart, or
//! stop. The client keeps connecting, says whether it's connected, and
//! subscribes again once it is: a chat's feed starts over with a reset.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};

/// The first wait before connecting again, doubled up to [`RETRY_MAX`].
const RETRY_MIN: Duration = Duration::from_millis(250);
/// The longest wait: how late the app notices an engine that started.
const RETRY_MAX: Duration = Duration::from_secs(4);

/// What a call answers while the engine is down.
pub const DOWN: &str = "Ludolph's engine isn't running";

/// Where a subscription's pushes go, on the client's task.
pub type Sink = Arc<dyn Fn(Value) + Send + Sync>;

enum Request {
    Call {
        tool: String,
        args: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Subscribe {
        id: u64,
        topic: &'static str,
        args: Value,
        sink: Sink,
    },
    Unsubscribe(u64),
}

/// A handle to the connection; clones share it.
#[derive(Clone)]
pub struct Client {
    requests: mpsc::UnboundedSender<Request>,
    next: Arc<AtomicU64>,
    up: watch::Receiver<bool>,
}

impl Client {
    /// Connect to the engine at `socket` from a task on the current runtime.
    pub fn start(socket: PathBuf) -> Self {
        let (requests, queue) = mpsc::unbounded_channel();
        let (up, watching) = watch::channel(false);
        tokio::spawn(run(socket, queue, up));
        Self {
            requests,
            next: Arc::new(AtomicU64::new(1)),
            up: watching,
        }
    }

    /// Whether the engine is connected, as it changes.
    pub fn up(&self) -> watch::Receiver<bool> {
        self.up.clone()
    }

    /// Call `tool` with `args`, and return its answer or why it failed.
    pub async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        let (reply, answer) = oneshot::channel();
        let request = Request::Call {
            tool: tool.to_owned(),
            args,
            reply,
        };
        self.requests.send(request).map_err(|_| DOWN.to_owned())?;
        answer.await.unwrap_or_else(|_| Err(DOWN.to_owned()))
    }

    /// Follow `topic` with `args`: each push goes to `sink`, and so does
    /// `{"error": …}` if the engine refuses it. It lasts until the handle
    /// drops.
    pub fn subscribe(&self, topic: &'static str, args: Value, sink: Sink) -> Subscription {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let _ = self.requests.send(Request::Subscribe {
            id,
            topic,
            args,
            sink,
        });
        Subscription {
            id,
            requests: self.requests.clone(),
        }
    }
}

/// A subscription, ended when it drops.
pub struct Subscription {
    id: u64,
    requests: mpsc::UnboundedSender<Request>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Unsubscribe(self.id));
    }
}

struct Followed {
    topic: &'static str,
    args: Value,
    sink: Sink,
}

/// Connect, serve, and connect again, until every [`Client`] is gone.
async fn run(
    socket: PathBuf,
    mut queue: mpsc::UnboundedReceiver<Request>,
    up: watch::Sender<bool>,
) {
    let mut followed: HashMap<u64, Followed> = HashMap::new();
    let mut wait = RETRY_MIN;
    loop {
        if let Ok(stream) = UnixStream::connect(&socket).await {
            wait = RETRY_MIN;
            up.send_replace(true);
            let open = serve(stream, &mut queue, &mut followed).await;
            up.send_replace(false);
            if !open {
                return;
            }
        }
        // Calls fail at once while the engine is down; subscriptions wait.
        let sleep = tokio::time::sleep(wait);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                request = queue.recv() => match request {
                    None => return,
                    Some(Request::Call { reply, .. }) => drop(reply.send(Err(DOWN.to_owned()))),
                    Some(Request::Subscribe { id, topic, args, sink }) => {
                        followed.insert(id, Followed { topic, args, sink });
                    }
                    Some(Request::Unsubscribe(id)) => drop(followed.remove(&id)),
                },
            }
        }
        wait = (wait * 2).min(RETRY_MAX);
    }
}

/// Serve requests on `stream` until it ends. Returns whether clients are
/// still there.
async fn serve(
    stream: UnixStream,
    queue: &mut mpsc::UnboundedReceiver<Request>,
    followed: &mut HashMap<u64, Followed>,
) -> bool {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut calls: HashMap<String, oneshot::Sender<Result<Value, String>>> = HashMap::new();
    let mut next_call = 0u64;
    let mut out = Vec::new();
    for (id, f) in followed.iter() {
        line(&mut out, &subscribe(*id, f));
    }
    let open = loop {
        if !out.is_empty() {
            if write.write_all(&out).await.is_err() {
                break true;
            }
            out.clear();
        }
        tokio::select! {
            request = queue.recv() => match request {
                None => break false,
                Some(Request::Call { tool, args, reply }) => {
                    next_call += 1;
                    let id = format!("c{next_call}");
                    line(&mut out, &json!({"id": id, "call": tool, "args": args}));
                    calls.insert(id, reply);
                }
                Some(Request::Subscribe { id, topic, args, sink }) => {
                    let f = Followed { topic, args, sink };
                    line(&mut out, &subscribe(id, &f));
                    followed.insert(id, f);
                }
                Some(Request::Unsubscribe(id)) => {
                    if followed.remove(&id).is_some() {
                        line(&mut out, &json!({"id": format!("u{id}"), "unsubscribe": sub_id(id)}));
                    }
                }
            },
            read = lines.next_line() => match read {
                Ok(Some(text)) => receive(&text, &mut calls, followed),
                Ok(None) | Err(_) => break true,
            },
        }
    };
    for (_, reply) in calls.drain() {
        let _ = reply.send(Err(DOWN.to_owned()));
    }
    open
}

fn sub_id(id: u64) -> String {
    format!("s{id}")
}

fn subscribe(id: u64, f: &Followed) -> Value {
    json!({"id": sub_id(id), "subscribe": f.topic, "args": f.args})
}

fn line(out: &mut Vec<u8>, record: &Value) {
    serde_json::to_writer(&mut *out, record).expect("JSON values serialize");
    out.push(b'\n');
}

/// Hand one record from the engine to the call or subscription it's for.
fn receive(
    text: &str,
    calls: &mut HashMap<String, oneshot::Sender<Result<Value, String>>>,
    followed: &mut HashMap<u64, Followed>,
) {
    let Ok(mut record) = serde_json::from_str::<Value>(text) else {
        eprintln!("the engine sent a line that isn't JSON");
        return;
    };
    let of = |key: &str| {
        record[key]
            .as_str()
            .and_then(|id| id.strip_prefix('s'))
            .and_then(|n| n.parse::<u64>().ok())
    };
    if record.get("event").is_some() {
        if let Some(f) = of("subscription").and_then(|id| followed.get(&id)) {
            (f.sink)(record);
        }
        return;
    }
    let id = record["id"].as_str().unwrap_or_default().to_owned();
    if let Some(reply) = calls.remove(&id) {
        let answer = match record.get_mut("error") {
            Some(error) => Err(error.as_str().unwrap_or("the engine failed").to_owned()),
            None => Ok(record["ok"].take()),
        };
        let _ = reply.send(answer);
    } else if let Some(error) = record.get("error").filter(|e| !e.is_null()) {
        // A subscription the engine refused, as for a chat that's gone.
        if let Some(f) = of("id").and_then(|id| followed.remove(&id)) {
            (f.sink)(json!({"error": error}));
        }
    }
}
