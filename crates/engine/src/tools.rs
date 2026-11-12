//! The tools the app and the phone call, by name, with JSON arguments and
//! answers in the shapes the phone app reads today (zeron-tools'
//! `connect/tools.rs`).

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::engine::{ChatInfo, Delivery, Engine, Status};

/// Chats `list_chats` returns without a `limit`, and at most.
const DEFAULT_CHATS: usize = 50;
const MAX_CHATS: usize = 500;
/// Messages `chat_view` returns without a `limit`, and at most.
const DEFAULT_MESSAGES: usize = 40;
const MAX_MESSAGES: usize = 500;

/// A tool, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ListChats,
    ListProjects,
    ChatView,
    SendMessage,
    CreateChat,
    InterruptChat,
    TakeOverChat,
}

impl FromStr for Tool {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self> {
        Ok(match name {
            "list_chats" => Self::ListChats,
            "list_projects" => Self::ListProjects,
            "chat_view" => Self::ChatView,
            "send_message" => Self::SendMessage,
            "create_chat" => Self::CreateChat,
            "interrupt_chat" => Self::InterruptChat,
            "take_over_chat" => Self::TakeOverChat,
            _ => bail!("no tool {name}"),
        })
    }
}

/// Run `tool` with `args`.
///
/// # Errors
///
/// Fails if the arguments don't fit the tool, or the tool fails.
pub async fn call(engine: &Engine, tool: Tool, args: Value) -> Result<Value> {
    match tool {
        Tool::ListChats => list_chats(engine, parse(args)?),
        Tool::ListProjects => Ok(list_projects(engine)),
        Tool::ChatView => chat_view(engine, parse(args)?).await,
        Tool::SendMessage => send_message(engine, parse(args)?).await,
        Tool::CreateChat => create_chat(engine, parse(args)?).await,
        Tool::InterruptChat => interrupt_chat(engine, parse(args)?).await,
        Tool::TakeOverChat => take_over_chat(engine, parse(args)?).await,
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T> {
    // A tool called without arguments gets none.
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).context("invalid arguments")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListChatsArgs {
    limit: Option<usize>,
}

/// The chats, most recently active first.
fn list_chats(engine: &Engine, args: ListChatsArgs) -> Result<Value> {
    let chats = engine.chats();
    let limit = args.limit.unwrap_or(DEFAULT_CHATS).clamp(1, MAX_CHATS);
    let home = home();
    let scratch = scratch_root(engine);
    let summaries: Vec<Value> = chats
        .iter()
        .take(limit)
        .map(|chat| summary(chat, &home, &scratch))
        .collect();
    Ok(json!({"total": chats.len(), "chats": summaries}))
}

/// The projects: every chat's folder, except the home and scratch folders.
fn list_projects(engine: &Engine) -> Value {
    let home = home();
    let scratch = scratch_root(engine);
    let mut folders: Vec<PathBuf> = engine
        .chats()
        .into_iter()
        .map(|chat| chat.cwd)
        .filter(|cwd| is_project(cwd, &home, &scratch))
        .collect();
    folders.sort();
    folders.dedup();
    let projects: Vec<Value> = folders
        .iter()
        .map(|path| {
            let mut project = project(path);
            project["git"] = json!(path.join(".git").exists());
            project
        })
        .collect();
    json!({"projects": projects})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewArgs {
    chat: String,
    limit: Option<usize>,
    /// The `version` the caller shows.
    known: Option<String>,
}

/// A chat and its latest messages. `read` is `null` when the caller already
/// shows this `version`.
async fn chat_view(engine: &Engine, args: ViewArgs) -> Result<Value> {
    let limit = args
        .limit
        .unwrap_or(DEFAULT_MESSAGES)
        .clamp(1, MAX_MESSAGES);
    let view = engine.view(&args.chat, limit).await?;
    let version = format!(
        "{}|{}|{}|{limit}",
        view.page.leaf.as_deref().unwrap_or_default(),
        view.revision,
        label(view.chat.status),
    );
    let read = (args.known.as_deref() != Some(version.as_str())).then(|| {
        json!({
            "total": view.page.total,
            "olderRemaining": view.page.older(),
            "pendingInput": null,
            "messages": view.page.messages,
            "turnChanges": [],
        })
    });
    Ok(json!({
        "chat": summary(&view.chat, &home(), &scratch_root(engine)),
        "version": version,
        "read": read,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    chat: String,
    text: String,
}

/// Send `text` to a chat: start a run, or steer the one that's going.
async fn send_message(engine: &Engine, args: SendArgs) -> Result<Value> {
    let text = args.text.trim();
    if text.is_empty() {
        bail!("text is empty");
    }
    let before = find(engine, &args.chat)?;
    let delivery = engine.send(&args.chat, text, Vec::new()).await?;
    Ok(json!({
        "chatId": args.chat,
        "title": before.summary.and_then(|s| s.title),
        "sent": sent(delivery, before.status),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateChatArgs {
    /// The project's folder; without one, the chat gets a scratch folder.
    project: Option<PathBuf>,
    prompt: Option<String>,
}

/// Create a chat, in a project or a scratch folder, and send it the first
/// prompt. If the chat was created but the prompt didn't go, `sendError`
/// says why: retrying the whole call would create a second chat.
async fn create_chat(engine: &Engine, args: CreateChatArgs) -> Result<Value> {
    let cwd = match &args.project {
        Some(project) => project.clone(),
        None => {
            let day = chrono::Local::now().format("%Y-%m-%d");
            let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
            let cwd = scratch_root(engine).join(format!("{day}-{tag}"));
            std::fs::create_dir_all(&cwd).with_context(|| format!("creating {}", cwd.display()))?;
            cwd
        }
    };
    let id = engine.create(&cwd).await?;
    let mut answer = json!({
        "chatId": id,
        "kind": "chat",
        "project": args.project.as_deref().map(project),
        "harness": "pi",
    });
    if let Some(prompt) = args
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        match engine.send(&id, prompt, Vec::new()).await {
            Ok(delivery) => answer["sent"] = sent(delivery, Status::Idle),
            Err(e) => answer["sendError"] = json!(format!("{e:#}")),
        }
    }
    Ok(answer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatArgs {
    chat: String,
}

async fn interrupt_chat(engine: &Engine, args: ChatArgs) -> Result<Value> {
    find(engine, &args.chat)?;
    engine.interrupt(&args.chat).await?;
    Ok(json!({"chatId": args.chat}))
}

async fn take_over_chat(engine: &Engine, args: ChatArgs) -> Result<Value> {
    engine.take_over(&args.chat).await?;
    Ok(json!({"chatId": args.chat}))
}

fn find(engine: &Engine, id: &str) -> Result<ChatInfo> {
    engine
        .chats()
        .into_iter()
        .find(|chat| chat.id == id)
        .with_context(|| format!("no chat {id}"))
}

/// A chat as `list_chats` describes it.
fn summary(chat: &ChatInfo, home: &Path, scratch: &Path) -> Value {
    let summary = chat.summary.as_ref();
    json!({
        "id": chat.id,
        "title": summary.and_then(|s| s.title.as_deref()),
        "project": is_project(&chat.cwd, home, scratch).then(|| project(&chat.cwd)),
        "cwd": chat.cwd,
        "harness": "pi",
        "archived": false,
        "status": label(chat.status),
        "openElsewhere": chat.elsewhere,
        "lastMessageAt": summary.and_then(|s| s.last_activity),
        "lastMessagePreview": summary.and_then(|s| s.last_prompt.as_deref()),
        "firstPrompt": summary.and_then(|s| s.first_prompt.as_deref()),
        "createdAt": chat.created,
    })
}

/// A project, named by its folder.
fn project(path: &Path) -> Value {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    json!({"id": path, "name": name, "path": path})
}

/// Whether chats in `cwd` belong to a project, rather than to "No project".
fn is_project(cwd: &Path, home: &Path, scratch: &Path) -> bool {
    cwd != home && !cwd.starts_with(scratch)
}

fn sent(delivery: Delivery, status: Status) -> Value {
    let delivery = match delivery {
        Delivery::Started => "run",
        Delivery::Queued => "steer",
        Delivery::Handled => "handled",
    };
    json!({"delivery": delivery, "chatStatusAtSend": label(status)})
}

/// A status as the phone app names it.
fn label(status: Status) -> &'static str {
    match status {
        Status::Idle => "idle",
        Status::Working => "working",
        Status::Failed => "errored",
        Status::Interrupted => "interrupted",
    }
}

fn scratch_root(engine: &Engine) -> PathBuf {
    engine.config().state_dir.join("scratch")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}
