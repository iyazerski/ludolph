use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(4);

#[cfg(not(windows))]
pub const DOWN: &str = "Ludolph's engine isn't running";
#[cfg(windows)]
pub const DOWN: &str =
    "Ludolph's engine in WSL isn't running. Install Ludolph in WSL, then try again";

/// How the app reaches the engine: its socket, which launchd keeps served.
#[cfg(target_os = "macos")]
pub struct Link(PathBuf);

/// How the app reaches the engine: `ludolph connect`, which starts the engine if it isn't
/// running.
#[cfg(not(target_os = "macos"))]
pub struct Link {
    program: PathBuf,
    args: &'static [&'static str],
}

impl Link {
    #[cfg(target_os = "macos")]
    pub fn new(state_dir: &Path) -> std::io::Result<Self> {
        Ok(Self(state_dir.join("engine.sock")))
    }

    #[cfg(target_os = "linux")]
    pub fn new(_state_dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            args: &["connect"],
        })
    }

    /// The engine runs in the default WSL distribution, where install.sh puts it in ~/.local.
    #[cfg(windows)]
    pub fn new(_state_dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            program: PathBuf::from("wsl.exe"),
            args: &["--cd", "~", "--exec", ".local/bin/ludolph", "connect"],
        })
    }

    /// A link to no engine, for tests.
    #[cfg(test)]
    pub fn nowhere() -> Self {
        #[cfg(target_os = "macos")]
        return Self(PathBuf::from("/nonexistent/engine.sock"));
        #[cfg(not(target_os = "macos"))]
        Self {
            program: PathBuf::from("/nonexistent/ludolph"),
            args: &[],
        }
    }

    #[cfg(target_os = "macos")]
    async fn connect(&self) -> Option<Connection> {
        let (read, write) = tokio::net::UnixStream::connect(&self.0)
            .await
            .ok()?
            .into_split();
        Some(Connection {
            read: Box::new(BufReader::new(read)),
            write: Box::new(write),
        })
    }

    #[cfg(not(target_os = "macos"))]
    async fn connect(&self) -> Option<Connection> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(self.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let mut bridge = command.spawn().ok()?;
        let mut read = BufReader::new(bridge.stdout.take()?);
        let write = bridge.stdin.take()?;
        // `connect` sends an empty line once the engine answers.
        let mut ready = String::new();
        match read.read_line(&mut ready).await {
            Ok(1..) if ready.trim().is_empty() => Some(Connection {
                read: Box::new(read),
                write: Box::new(write),
                _bridge: bridge,
            }),
            _ => None,
        }
    }
}

struct Connection {
    read: Box<dyn AsyncBufRead + Send + Unpin>,
    write: Box<dyn AsyncWrite + Send + Unpin>,
    /// Killed when the connection drops.
    #[cfg(not(target_os = "macos"))]
    _bridge: tokio::process::Child,
}

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

#[derive(Clone)]
pub struct Client {
    requests: mpsc::UnboundedSender<Request>,
    next: Arc<AtomicU64>,
    up: watch::Receiver<bool>,
}

impl Client {
    pub fn start(link: Link) -> Self {
        let (requests, queue) = mpsc::unbounded_channel();
        let (up, watching) = watch::channel(false);
        tokio::spawn(run(link, queue, up));
        Self {
            requests,
            next: Arc::new(AtomicU64::new(1)),
            up: watching,
        }
    }

    pub fn up(&self) -> watch::Receiver<bool> {
        self.up.clone()
    }

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

async fn run(link: Link, mut queue: mpsc::UnboundedReceiver<Request>, up: watch::Sender<bool>) {
    let mut followed: HashMap<u64, Followed> = HashMap::new();
    let mut wait = RETRY_MIN;
    loop {
        if let Some(connection) = link.connect().await {
            wait = RETRY_MIN;
            up.send_replace(true);
            let open = serve(connection, &mut queue, &mut followed).await;
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

async fn serve(
    connection: Connection,
    queue: &mut mpsc::UnboundedReceiver<Request>,
    followed: &mut HashMap<u64, Followed>,
) -> bool {
    let mut write = connection.write;
    let mut lines = connection.read.lines();
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
    } else if let Some(error) = record.get("error").filter(|e| !e.is_null())
        && let Some(f) = of("id").and_then(|id| followed.remove(&id))
    {
        (f.sink)(json!({"error": error}));
    }
}
