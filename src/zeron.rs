//! `ludolph import-zeron`: give the pi sessions Zeron ran their chats'
//! titles and archive state. Zeron must be running, since its chat list is
//! only reachable through its loopback RPC.
//!
//! A chat's session is the one its journal says started. Chats without one
//! in the journal get the only session that started in their folder shortly
//! after them.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use ludolph_engine::server;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

/// Zeron's loopback RPC.
const IPC: &str = "ws://127.0.0.1:27654";
/// How long Zeron gets to send its chat list.
const TIMEOUT: Duration = Duration::from_secs(15);
/// How long after a chat its session may have started. Zeron starts pi at
/// once: the sessions its journals name started at most 20 s after.
const START_WINDOW: chrono::TimeDelta = chrono::TimeDelta::seconds(120);
/// The most chats `list_chats` returns in one call.
const MAX_CHATS: usize = 500;

/// A chat in Zeron's list.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZeronChat {
    id: String,
    title: String,
    archived: bool,
    /// Zeron writes the home folder as `~`.
    cwd: String,
    created_at: DateTime<Utc>,
    config: ZeronConfig,
}

#[derive(Deserialize)]
struct ZeronConfig {
    harness: String,
}

/// A pi session, as the engine lists it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id: String,
    cwd: PathBuf,
    created_at: DateTime<Utc>,
}

/// Import Zeron's pi chats into the engine at `state_dir`, and report how
/// many found their session.
///
/// # Errors
///
/// Fails if Zeron or the engine isn't running.
pub async fn import(home: &Path, state_dir: &Path) -> Result<String> {
    let zeron = home.join(".zeron");
    let chats = zeron_chats(&zeron).await?;
    let socket = server::path(state_dir);
    let mut sessions = Vec::new();
    for archived in [false, true] {
        let args = json!({"limit": MAX_CHATS, "archived": archived});
        let mut listed = server::request(&socket, "list_chats", args).await?;
        if listed["total"]
            .as_u64()
            .is_some_and(|n| n > MAX_CHATS as u64)
        {
            bail!("the engine lists more than {MAX_CHATS} chats, more than the import reads");
        }
        sessions.extend(serde_json::from_value::<Vec<Session>>(
            listed["chats"].take(),
        )?);
    }

    let pi: Vec<_> = chats.iter().filter(|c| c.config.harness == "pi").collect();
    let known: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let journals = journal_dirs(&zeron);
    let mut matched: Vec<(&ZeronChat, String)> = Vec::new();
    let mut rest = Vec::new();
    for chat in pi {
        match started_session(&journals, &chat.id)? {
            Some(id) if known.contains(id.as_str()) => matched.push((chat, id)),
            _ => rest.push(chat),
        }
    }
    let mut taken: HashSet<String> = matched.iter().map(|(_, id)| id.clone()).collect();
    for chat in rest {
        let cwd = folder(home, &chat.cwd);
        let after = |s: &&Session| {
            let delay = s.created_at - chat.created_at;
            s.cwd == cwd && delay >= chrono::TimeDelta::zero() && delay <= START_WINDOW
        };
        let fits: Vec<_> = sessions
            .iter()
            .filter(after)
            .filter(|s| !taken.contains(&s.id))
            .collect();
        if let [session] = fits[..] {
            taken.insert(session.id.clone());
            matched.push((chat, session.id.clone()));
        }
    }

    let total = chats.iter().filter(|c| c.config.harness == "pi").count();
    let others = chats.len() - total;
    let imports: Vec<Value> = matched
        .iter()
        .map(|(chat, id)| json!({"chat": id, "title": chat.title, "archived": chat.archived}))
        .collect();
    let imported = server::request(&socket, "import_chats", json!({"chats": imports})).await?;
    let found = imported["imported"].as_u64().unwrap_or_default();
    let lost = total as u64 - found;
    Ok(format!(
        "Imported {found} of Zeron's {total} pi chats. {lost} have no pi session left, and {others} ran in other harnesses."
    ))
}

/// Zeron's chats: the first item of its `WatchChats` stream.
async fn zeron_chats(zeron: &Path) -> Result<Vec<ZeronChat>> {
    let token = zeron.join("ipc-token");
    let token = std::fs::read_to_string(&token)
        .with_context(|| format!("reading {}: is Zeron running?", token.display()))?;
    let mut request = IPC.into_client_request()?;
    let bearer = HeaderValue::try_from(format!("Bearer {}", token.trim()))?;
    request.headers_mut().insert("authorization", bearer);
    let read = async {
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .with_context(|| format!("no Zeron at {IPC}: is it running?"))?;
        let call = json!({"id": 1, "method": "WatchChats", "params": {}});
        socket.send(Message::text(call.to_string())).await?;
        while let Some(message) = socket.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let mut frame: Value = serde_json::from_str(&text)?;
            if let Some(error) = frame["err"].as_str() {
                bail!("Zeron's WatchChats: {error}");
            }
            if frame["id"] == 1 && !frame["item"].is_null() {
                return Ok(serde_json::from_value(frame["item"].take())?);
            }
        }
        bail!("Zeron closed the connection before listing its chats")
    };
    tokio::time::timeout(TIMEOUT, read)
        .await
        .context("Zeron didn't list its chats")?
}

/// Where Zeron keeps chat journals: a local profile's, and each signed-in
/// account's.
fn journal_dirs(zeron: &Path) -> Vec<PathBuf> {
    let children = |dir: PathBuf| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
    };
    let profiles = children(zeron.join("profiles"));
    let accounts = children(zeron.join("orgs")).flat_map(children);
    profiles
        .chain(accounts)
        .map(|dir| dir.join("journals"))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// The pi session chat `id`'s journal says started, if any.
fn started_session(journals: &[PathBuf], id: &str) -> Result<Option<String>> {
    for dir in journals {
        let path = dir.join(format!("{id}.jsonl"));
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in BufReader::new(file).lines() {
            let line = line.with_context(|| format!("reading {}", path.display()))?;
            // Most lines are messages: only parse the one that can match.
            if !line.contains("\"sessionStarted\"") {
                continue;
            }
            let record: Record = serde_json::from_str(&line)
                .with_context(|| format!("reading {}", path.display()))?;
            if record.event.kind == "sessionStarted" {
                return Ok(record.event.session_id);
            }
        }
    }
    Ok(None)
}

/// A line of a journal.
#[derive(Deserialize)]
struct Record {
    event: Event,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    session_id: Option<String>,
}

/// Zeron's folder, as pi wrote it: `~` expanded and links resolved.
fn folder(home: &Path, cwd: &str) -> PathBuf {
    let path = match cwd.strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(cwd),
    };
    std::fs::canonicalize(&path).unwrap_or(path)
}
