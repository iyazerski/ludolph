use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Result, anyhow};
use axum::extract::ws::{Message, WebSocket};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use futures_util::StreamExt;
use ludolph_relay_proto::{PROTOCOL, PairStart, Paired};
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::hub::close;
use crate::store::Machine;

pub const CODE_TTL: Duration = Duration::from_secs(600);
const MAX_CODES: usize = 32;
const START_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(25);
const MAX_NAME: usize = 60;

struct Waiting {
    name: String,
    machine: Option<Machine>,
    at: Instant,
    /// Hands the Mac its new token, or `None` if it has one.
    linked: oneshot::Sender<Option<String>>,
}

pub struct Code {
    pub name: String,
    pub known: bool,
}

pub struct Claim {
    pub name: String,
    pub machine: Option<Machine>,
    linked: oneshot::Sender<Option<String>>,
}

impl Claim {
    pub fn finish(self, token: Option<String>) -> bool {
        self.linked.send(token).is_ok()
    }
}

pub struct Pairings {
    codes: Mutex<HashMap<String, Waiting>>,
    rng: SystemRandom,
}

impl Default for Pairings {
    fn default() -> Self {
        Self {
            codes: Mutex::default(),
            rng: SystemRandom::new(),
        }
    }
}

impl Pairings {
    pub fn code(&self, code: &str) -> Option<Code> {
        let codes = self.lock();
        let waiting = codes.get(code).filter(|w| live(w))?;
        Some(Code {
            name: waiting.name.clone(),
            known: waiting.machine.is_some(),
        })
    }

    pub fn claim(&self, code: &str) -> Option<Claim> {
        let waiting = self.lock().remove(code).filter(live)?;
        Some(Claim {
            name: waiting.name,
            machine: waiting.machine,
            linked: waiting.linked,
        })
    }

    pub async fn serve(&self, mut socket: WebSocket, machine: Option<Machine>) {
        if let Err(e) = self.serve_inner(&mut socket, machine).await {
            eprintln!("pairing: {e:#}");
            let _ = socket.send(close(&e.to_string())).await;
        }
    }

    async fn serve_inner(&self, socket: &mut WebSocket, machine: Option<Machine>) -> Result<()> {
        let start = tokio::time::timeout(START_TIMEOUT, socket.next()).await;
        let Ok(Some(Ok(Message::Text(body)))) = start else {
            return Err(anyhow!("the Mac didn't say who it is"));
        };
        let start: PairStart = serde_json::from_str(&body)?;
        if start.protocol != PROTOCOL {
            return Err(anyhow!(
                "this Mac speaks another relay protocol; update Ludolph or the relay"
            ));
        }
        let name = start.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(anyhow!("a Mac's name must be 1 to {MAX_NAME} characters"));
        }
        let (code, linked) = self.start(name, machine)?;
        let sent = Paired::Code {
            code: code.clone(),
            expires_in: CODE_TTL.as_secs(),
        };
        socket.send(text(&sent)?).await?;
        let deadline = Instant::now() + CODE_TTL;
        let mut ping = tokio::time::interval(PING_EVERY);
        let mut linked = std::pin::pin!(linked);
        let result = loop {
            tokio::select! {
                token = &mut linked => match token {
                    Ok(token) => {
                        socket.send(text(&Paired::Linked { token })?).await?;
                        let _ = socket.send(Message::Close(None)).await;
                        break Ok(());
                    }
                    Err(_) => break Err(anyhow!("the link didn't finish; try again")),
                },
                () = tokio::time::sleep_until(deadline) => break Err(anyhow!("the code expired")),
                _ = ping.tick() => socket.send(Message::Ping(Default::default())).await?,
                frame = socket.next() => match frame {
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break Ok(()),
                    Some(Ok(_)) => {}
                },
            }
        };
        self.lock().remove(&code);
        result
    }

    fn start(
        &self,
        name: &str,
        machine: Option<Machine>,
    ) -> Result<(String, oneshot::Receiver<Option<String>>)> {
        let mut bytes = [0u8; 16];
        self.rng
            .fill(&mut bytes)
            .map_err(|_| anyhow!("making a code"))?;
        let code = B64.encode(bytes);
        let (linked, wait) = oneshot::channel();
        let mut codes = self.lock();
        codes.retain(|_, w| live(w));
        while codes.len() >= MAX_CODES {
            let Some(oldest) = codes
                .iter()
                .min_by_key(|(_, w)| w.at)
                .map(|(code, _)| code.clone())
            else {
                break;
            };
            codes.remove(&oldest);
        }
        let waiting = Waiting {
            name: name.to_owned(),
            machine,
            at: Instant::now(),
            linked,
        };
        codes.insert(code.clone(), waiting);
        Ok((code, wait))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Waiting>> {
        self.codes
            .lock()
            .expect("the pairing codes lock is poisoned")
    }
}

fn live(waiting: &Waiting) -> bool {
    waiting.at.elapsed() < CODE_TTL && !waiting.linked.is_closed()
}

fn text(paired: &Paired) -> Result<Message> {
    Ok(Message::Text(serde_json::to_string(paired)?.into()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::WebSocketUpgrade;
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as Frame;

    use super::*;

    type MacSocket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn start(pairings: &Arc<Pairings>, name: &str) -> MacSocket {
        let serve = pairings.clone();
        let app = axum::Router::new().route(
            "/pair",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let pairings = serve.clone();
                async move {
                    ws.on_upgrade(move |socket| async move { pairings.serve(socket, None).await })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/pair"))
            .await
            .unwrap();
        let start = PairStart {
            protocol: PROTOCOL,
            name: name.into(),
        };
        socket
            .send(Frame::text(serde_json::to_string(&start).unwrap()))
            .await
            .unwrap();
        socket
    }

    async fn next(socket: &mut MacSocket) -> Option<Paired> {
        loop {
            match socket.next().await? {
                Ok(Frame::Text(body)) => return Some(serde_json::from_str(&body).unwrap()),
                Ok(Frame::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    #[tokio::test]
    async fn a_code_links_its_mac_once() {
        let pairings = Arc::new(Pairings::default());
        let mut mac = start(&pairings, "Work Mac").await;
        let Some(Paired::Code { code, expires_in }) = next(&mut mac).await else {
            panic!("no code");
        };
        assert_eq!(expires_in, CODE_TTL.as_secs());
        let shown = pairings.code(&code).unwrap();
        assert_eq!((shown.name.as_str(), shown.known), ("Work Mac", false));
        assert!(pairings.code("guess").is_none());

        let claim = pairings.claim(&code).unwrap();
        assert!(pairings.claim(&code).is_none(), "single-use");
        assert!(claim.finish(Some("t0ken".into())));
        let linked = Paired::Linked {
            token: Some("t0ken".into()),
        };
        assert_eq!(next(&mut mac).await, Some(linked));
        assert_eq!(next(&mut mac).await, None);
    }

    #[tokio::test]
    async fn a_mac_that_stops_waiting_takes_its_code_away() {
        let pairings = Arc::new(Pairings::default());
        let mut mac = start(&pairings, "Work Mac").await;
        let Some(Paired::Code { code, .. }) = next(&mut mac).await else {
            panic!("no code");
        };
        mac.close(None).await.unwrap();
        for _ in 0..100 {
            if pairings.code(&code).is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the code still works");
    }
}
