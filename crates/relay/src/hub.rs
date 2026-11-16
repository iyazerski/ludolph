use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Result, bail};
use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use ludolph_relay_proto::{Battery, Down, PROTOCOL, Sealed, Up};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::store::Machine;

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Over two of the Mac's pings (`PING_EVERY`). Until then, calls to a Mac
/// whose link died quietly wait out their timeout.
const SILENCE_LIMIT: Duration = Duration::from_secs(40);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_QUEUE: usize = 64;
/// A lagging phone must reconnect: dropping individual frames corrupts its page.
const WATCH_QUEUE: usize = 32;

pub type Answer = Result<String, String>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;
type Watches = Arc<Mutex<HashMap<u64, mpsc::Sender<Answer>>>>;

struct Link {
    conn: u64,
    calls: mpsc::Sender<Down>,
    pending: Pending,
    watches: Watches,
    version: String,
    battery: Option<Battery>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub online: bool,
    pub version: Option<String>,
    pub battery: Option<Battery>,
}

/// A chat the phone follows on a Mac: its frames, until the Mac goes away.
/// Dropping it tells the Mac to stop.
pub struct Watch {
    pub frames: mpsc::Receiver<Answer>,
    id: u64,
    watches: Watches,
    calls: mpsc::WeakSender<Down>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        lock(&self.watches).remove(&self.id);
        // A full queue only delays it: the Mac's next frame for this watch
        // gets an unwatch back.
        if let Some(calls) = self.calls.upgrade() {
            let _ = calls.try_send(Down::Unwatch { id: self.id });
        }
    }
}

#[derive(Default)]
pub struct Hub {
    links: Mutex<HashMap<String, Link>>,
    next: AtomicU64,
}

impl Hub {
    pub fn status(&self, machine: &str) -> Status {
        match self.lock().get(machine) {
            Some(link) => Status {
                online: true,
                version: Some(link.version.clone()),
                battery: link.battery,
            },
            None => Status::default(),
        }
    }

    /// The phone's retry policy depends on these exact error strings (app.js RETRY).
    pub async fn call(&self, machine: &str, sealed: Sealed) -> Answer {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (calls, pending) = {
            let links = self.lock();
            let link = links.get(machine).ok_or("this Mac is offline")?;
            (link.calls.clone(), link.pending.clone())
        };
        let (tx, rx) = oneshot::channel();
        lock(&pending).insert(id, tx);
        let _waiting = Waiting {
            pending: &pending,
            id,
        };
        let exchange = async move {
            let sent = calls.send(Down::Call { id, sealed }).await;
            // Don't keep the link's call queue open: a replaced connection
            // ends when its last sender is gone.
            drop(calls);
            if sent.is_err() {
                return Err("this Mac went offline".into());
            }
            rx.await
                .unwrap_or_else(|_| Err("this Mac went offline".into()))
        };
        tokio::time::timeout(CALL_TIMEOUT, exchange)
            .await
            .unwrap_or_else(|_| Err("this Mac didn't answer in time".into()))
    }

    pub async fn watch(&self, machine: &str, sealed: Sealed) -> Result<Watch, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (calls, watches) = {
            let links = self.lock();
            let link = links.get(machine).ok_or("this Mac is offline")?;
            (link.calls.clone(), link.watches.clone())
        };
        let (tx, frames) = mpsc::channel(WATCH_QUEUE);
        lock(&watches).insert(id, tx);
        let watch = Watch {
            frames,
            id,
            watches,
            calls: calls.downgrade(),
        };
        match tokio::time::timeout(CALL_TIMEOUT, calls.send(Down::Watch { id, sealed })).await {
            Ok(Ok(())) => Ok(watch),
            Ok(Err(_)) => Err("this Mac went offline".into()),
            Err(_) => Err("this Mac didn't answer in time".into()),
        }
    }

    pub fn drop_machine(&self, machine: &str) {
        self.lock().remove(machine);
    }

    /// Recheck known after hello: the Mac may be removed while its handshake is in flight.
    pub async fn serve(
        self: Arc<Self>,
        machine: Machine,
        socket: WebSocket,
        known: impl Fn(&str) -> bool + Send,
        notify: impl Fn(&Machine, Vec<Sealed>) + Send,
        forget: impl Fn(&Machine) + Send,
    ) {
        if let Err(e) = self
            .serve_inner(&machine, socket, known, notify, forget)
            .await
        {
            eprintln!("{}: {e:#}", machine.name);
        }
    }

    async fn serve_inner(
        &self,
        machine: &Machine,
        socket: WebSocket,
        known: impl Fn(&str) -> bool + Send,
        notify: impl Fn(&Machine, Vec<Sealed>) + Send,
        forget: impl Fn(&Machine) + Send,
    ) -> Result<()> {
        let (mut sink, mut stream) = socket.split();
        let version = hello(&mut sink, &mut stream).await?;
        let conn = self.next.fetch_add(1, Ordering::Relaxed);
        let (calls_tx, mut calls) = mpsc::channel::<Down>(CALL_QUEUE);
        let pending = Pending::default();
        let watches = Watches::default();
        {
            // Checked under the hub's lock: a removal either happens first and
            // is seen here, or its `drop_machine` runs after this insert.
            let mut links = self.lock();
            if !known(&machine.id) {
                drop(links);
                let _ = sink.send(close("this Mac was removed")).await;
                bail!("removed while connecting");
            }
            links.insert(
                machine.id.clone(),
                Link {
                    conn,
                    calls: calls_tx,
                    pending: pending.clone(),
                    watches: watches.clone(),
                    version: version.clone(),
                    battery: None,
                },
            );
        }
        eprintln!("{} connected (Ludolph {version})", machine.name);
        let result = async {
            // Outgoing calls must not postpone this, so it's a fixed deadline
            // that only frames from the Mac move.
            let mut heard = Instant::now();
            loop {
                tokio::select! {
                    () = tokio::time::sleep_until(heard + SILENCE_LIMIT) => bail!("the Mac went silent"),
                    frame = stream.next() => {
                        heard = Instant::now();
                        match frame {
                            Some(Ok(Message::Text(body))) => match serde_json::from_str(&body) {
                                Ok(Up::Forget) => {
                                    forget(machine);
                                    let _ = sink.send(close("unlinked")).await;
                                    return Ok(());
                                }
                                Ok(up) => {
                                    if let Some(down) = self.receive(machine, conn, &pending, &watches, up, &notify) {
                                        sink.send(Message::Text(serde_json::to_string(&down)?.into())).await?;
                                    }
                                }
                                Err(e) => eprintln!("{}: skipped a bad message: {e}", machine.name),
                            },
                            Some(Ok(Message::Close(_))) | None => return Ok(()),
                            Some(Ok(_)) => {}
                            Some(Err(e)) => return Err(e.into()),
                        }
                    }
                    call = calls.recv() => match call {
                        Some(call) => sink.send(Message::Text(serde_json::to_string(&call)?.into())).await?,
                        None => {
                            let _ = sink.send(close("replaced")).await;
                            return Ok(());
                        }
                    },
                }
            }
        }
        .await;
        // Unlist before failing waiters so new calls cannot enter the dead connection.
        {
            let mut links = self.lock();
            if links.get(&machine.id).is_some_and(|l| l.conn == conn) {
                links.remove(&machine.id);
            }
        }
        calls.close();
        lock(&pending).clear();
        lock(&watches).clear();
        eprintln!("{} disconnected", machine.name);
        result
    }

    fn receive(
        &self,
        machine: &Machine,
        conn: u64,
        pending: &Pending,
        watches: &Watches,
        up: Up,
        notify: &impl Fn(&Machine, Vec<Sealed>),
    ) -> Option<Down> {
        match up {
            Up::Reply { id, result } => {
                if let Some(waiter) = lock(pending).remove(&id) {
                    let _ = waiter.send(result);
                }
            }
            Up::Frame { id, frame } => {
                let mut watches = lock(watches);
                let Some(phone) = watches.get(&id) else {
                    // The phone left and its unwatch didn't fit in the queue.
                    return Some(Down::Unwatch { id });
                };
                // A phone too far behind loses the stream: a skipped frame
                // would leave its page wrong.
                if phone.try_send(frame).is_err() {
                    watches.remove(&id);
                    return Some(Down::Unwatch { id });
                }
            }
            Up::Notify { notices } => notify(machine, notices),
            Up::Power { battery } => self.update_link(machine, conn, |link| link.battery = battery),
            Up::Hello { .. } | Up::Forget => {}
        }
        None
    }

    /// Ignore updates from replaced connections.
    fn update_link(&self, machine: &Machine, conn: u64, update: impl FnOnce(&mut Link)) {
        if let Some(link) = self.lock().get_mut(&machine.id).filter(|l| l.conn == conn) {
            update(link);
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Link>> {
        lock(&self.links)
    }
}

async fn hello(
    sink: &mut SplitSink<WebSocket, Message>,
    stream: &mut SplitStream<WebSocket>,
) -> Result<String> {
    let Ok(Some(Ok(Message::Text(body)))) =
        tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await
    else {
        bail!("no hello");
    };
    match serde_json::from_str(&body)? {
        Up::Hello { protocol, version } if protocol == PROTOCOL => Ok(version),
        Up::Hello { version, .. } => {
            let reason = format!(
                "Ludolph {version} speaks another relay protocol; update Ludolph or the relay"
            );
            let _ = sink.send(close(&reason)).await;
            bail!(reason);
        }
        _ => bail!("expected a hello"),
    }
}

struct Waiting<'a> {
    pending: &'a Pending,
    id: u64,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        lock(self.pending).remove(&self.id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("the hub lock is poisoned")
}

pub fn close(reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code: close_code::POLICY,
        reason: reason.into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::WebSocketUpgrade;
    use tokio_tungstenite::tungstenite::Message as Frame;

    type MacSocket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn connect(hub: &Arc<Hub>, known: bool) -> MacSocket {
        let mac = Machine {
            id: "m".into(),
            name: "Mac".into(),
        };
        let serve = hub.clone();
        let app = axum::Router::new().route(
            "/connect",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let (hub, mac) = (serve.clone(), mac.clone());
                async move {
                    ws.on_upgrade(move |socket| {
                        hub.serve(mac, socket, move |_| known, |_, _| {}, |_| {})
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/connect"))
            .await
            .unwrap();
        let hello = Up::Hello {
            protocol: PROTOCOL,
            version: "1".into(),
        };
        socket
            .send(Frame::text(serde_json::to_string(&hello).unwrap()))
            .await
            .unwrap();
        socket
    }

    #[tokio::test]
    async fn calls_fail_at_once_when_the_mac_disconnects() {
        let hub = Arc::new(Hub::default());
        let mut socket = connect(&hub, true).await;
        while !hub.status("m").online {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let caller = hub.clone();
        let call = tokio::spawn(async move { caller.call("m", sealed("call")).await });
        let Some(Ok(Frame::Text(body))) = socket.next().await else {
            panic!("the Mac got no call");
        };
        assert!(
            matches!(serde_json::from_str::<Down>(&body).unwrap(), Down::Call { sealed, .. } if sealed.data == "call"),
            "{body}"
        );

        drop(socket);
        let result = tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .expect("the call waited for its timeout")
            .unwrap();
        assert_eq!(result, Err("this Mac went offline".into()));
        assert!(!hub.status("m").online);
    }

    fn sealed(data: &str) -> Sealed {
        Sealed {
            phone: "p".into(),
            data: data.into(),
        }
    }

    async fn to_mac(socket: &mut MacSocket) -> Down {
        let Some(Ok(Frame::Text(body))) = socket.next().await else {
            panic!("the Mac got nothing");
        };
        serde_json::from_str(&body).unwrap()
    }

    #[tokio::test]
    async fn a_followed_chat_streams_to_the_phone_until_it_leaves() {
        let hub = Arc::new(Hub::default());
        let mut socket = connect(&hub, true).await;
        while !hub.status("m").online {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut watch = hub.watch("m", sealed("c")).await.unwrap();
        let Down::Watch { id, sealed: asked } = to_mac(&mut socket).await else {
            panic!("the Mac wasn't asked to follow the chat");
        };
        assert_eq!(asked.data, "c");
        let frames: [Answer; 2] = [Ok("one".into()), Err("gone".into())];
        for frame in frames.clone() {
            let up = Up::Frame { id, frame };
            socket
                .send(Frame::text(serde_json::to_string(&up).unwrap()))
                .await
                .unwrap();
        }
        for frame in frames {
            assert_eq!(watch.frames.recv().await, Some(frame));
        }

        drop(watch);
        assert_eq!(to_mac(&mut socket).await, Down::Unwatch { id });
        let late = Up::Frame {
            id,
            frame: Ok("late".into()),
        };
        socket
            .send(Frame::text(serde_json::to_string(&late).unwrap()))
            .await
            .unwrap();
        assert_eq!(to_mac(&mut socket).await, Down::Unwatch { id });

        let mut watch = hub.watch("m", sealed("c")).await.unwrap();
        drop(socket);
        let ended = tokio::time::timeout(Duration::from_secs(5), watch.frames.recv()).await;
        assert_eq!(ended, Ok(None));
    }

    #[tokio::test]
    async fn a_mac_removed_while_connecting_is_turned_away() {
        let hub = Arc::new(Hub::default());
        let mut socket = connect(&hub, false).await;
        let closed = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the relay kept the removed Mac");
        assert!(
            matches!(closed, Some(Ok(Frame::Close(_))) | None),
            "{closed:?}"
        );
        assert!(!hub.status("m").online);
    }
}
