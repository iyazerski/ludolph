//! The tools the app and the phone call, by name, with JSON arguments and
//! answers in the shapes the phone app reads today (zeron-tools'
//! `connect/tools.rs`). The settings tools are the app's alone.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::engine::{
    ChatInfo, Composer, Config, Delivery, Dialog, Engine, Question, Reply, Status,
};
use crate::repo;
use crate::transcript::{Message, Role};
use crate::turns::Counts;

/// Chats `list_chats` returns without a `limit`, and at most.
const DEFAULT_CHATS: usize = 50;
const MAX_CHATS: usize = 500;
/// Chats `list_chat_groups` returns per group without a `limit`.
const DEFAULT_GROUP_CHATS: usize = 10;
/// Messages `chat_view` returns without a `limit`, and at most.
const DEFAULT_MESSAGES: usize = 40;
const MAX_MESSAGES: usize = 500;

/// A tool, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ListChats,
    ListChatGroups,
    ListProjects,
    ChatView,
    SendMessage,
    CreateChat,
    InterruptChat,
    TakeOverChat,
    RespondToInput,
    ArchiveChat,
    TurnChanges,
    ListHarnesses,
    ListModels,
    SetChatModel,
    ComposerInfo,
    ListSkills,
    PolishDictation,
    GetSettings,
    SetSettings,
    ImportChats,
}

impl Tool {
    /// Whether the phone may call it. The phone can't change settings, so a
    /// relay can't, say, point dictation at a model of its choosing, nor
    /// rewrite every title with an import.
    pub fn for_phone(self) -> bool {
        !matches!(
            self,
            Self::GetSettings | Self::SetSettings | Self::ImportChats
        )
    }
}

impl FromStr for Tool {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self> {
        Ok(match name {
            "list_chats" => Self::ListChats,
            "list_chat_groups" => Self::ListChatGroups,
            "list_projects" => Self::ListProjects,
            "chat_view" => Self::ChatView,
            "send_message" => Self::SendMessage,
            "create_chat" => Self::CreateChat,
            "interrupt_chat" => Self::InterruptChat,
            "take_over_chat" => Self::TakeOverChat,
            "respond_to_input" => Self::RespondToInput,
            "archive_chat" => Self::ArchiveChat,
            "turn_changes" => Self::TurnChanges,
            "list_harnesses" => Self::ListHarnesses,
            "list_models" => Self::ListModels,
            "set_chat_model" => Self::SetChatModel,
            "composer_info" => Self::ComposerInfo,
            "list_skills" => Self::ListSkills,
            "polish_dictation" => Self::PolishDictation,
            "get_settings" => Self::GetSettings,
            "set_settings" => Self::SetSettings,
            "import_chats" => Self::ImportChats,
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
        Tool::ListChatGroups => list_chat_groups(engine, parse(args)?),
        Tool::ListProjects => Ok(list_projects(engine)),
        Tool::ChatView => chat_view(engine, parse(args)?).await,
        Tool::SendMessage => send_message(engine, parse(args)?).await,
        Tool::CreateChat => create_chat(engine, parse(args)?).await,
        Tool::InterruptChat => interrupt_chat(engine, parse(args)?).await,
        Tool::TakeOverChat => take_over_chat(engine, parse(args)?).await,
        Tool::RespondToInput => respond_to_input(engine, parse(args)?).await,
        Tool::ArchiveChat => archive_chat(engine, parse(args)?),
        Tool::TurnChanges => turn_changes(engine, parse(args)?).await,
        Tool::ListHarnesses => Ok(list_harnesses()),
        Tool::ListModels => list_models(engine, parse(args)?).await,
        Tool::SetChatModel => set_chat_model(engine, parse(args)?).await,
        Tool::ComposerInfo => composer_info(engine, parse(args)?).await,
        Tool::ListSkills => list_skills(engine, parse(args)?).await,
        Tool::PolishDictation => polish_dictation(engine, parse(args)?).await,
        Tool::GetSettings => Ok(serde_json::to_value(engine.settings())?),
        // All of them at once: a field left out takes its default.
        Tool::SetSettings => Ok(serde_json::to_value(
            engine.set_settings(parse(args)?).await?,
        )?),
        Tool::ImportChats => import_chats(engine, parse(args)?),
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
    /// List the archived chats instead.
    #[serde(default)]
    archived: bool,
}

/// The chats that aren't archived, or only the archived ones, most recently
/// active first.
fn list_chats(engine: &Engine, args: ListChatsArgs) -> Result<Value> {
    let mut chats = engine.chats();
    chats.retain(|chat| chat.archived == args.archived);
    let limit = args.limit.unwrap_or(DEFAULT_CHATS).clamp(1, MAX_CHATS);
    let summaries: Vec<Value> = chats
        .iter()
        .take(limit)
        .map(|chat| summary(chat, engine.config()))
        .collect();
    Ok(json!({"total": chats.len(), "chats": summaries}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListChatGroupsArgs {
    /// Chats per group.
    limit: Option<usize>,
    /// Groups the user expanded, with their own limits.
    #[serde(default)]
    more: Vec<GroupLimit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupLimit {
    /// The project's id, or none for "No project".
    project: Option<PathBuf>,
    limit: usize,
}

/// The chats that aren't archived, by project, with "No project" as one
/// group. Groups come most recently active first, each with its newest
/// chats and its total, so a list can offer to show more.
fn list_chat_groups(engine: &Engine, args: ListChatGroupsArgs) -> Result<Value> {
    let config = engine.config();
    let default = args
        .limit
        .unwrap_or(DEFAULT_GROUP_CHATS)
        .clamp(1, MAX_CHATS);
    let mut groups: Vec<(Option<PathBuf>, Vec<ChatInfo>)> = Vec::new();
    let mut positions: HashMap<Option<PathBuf>, usize> = HashMap::new();
    for chat in engine.chats().into_iter().filter(|chat| !chat.archived) {
        let project = config.is_project(&chat.cwd).then(|| chat.cwd.clone());
        let at = *positions.entry(project.clone()).or_insert_with(|| {
            groups.push((project, Vec::new()));
            groups.len() - 1
        });
        groups[at].1.push(chat);
    }
    let groups: Vec<Value> = groups
        .iter()
        .map(|(project, chats)| {
            let more = args.more.iter().find(|more| more.project == *project);
            let limit = more.map_or(default, |more| more.limit.clamp(1, MAX_CHATS));
            let page: Vec<Value> = chats
                .iter()
                .take(limit)
                .map(|chat| summary(chat, config))
                .collect();
            json!({
                "project": project.as_deref().map(self::project),
                "total": chats.len(),
                "chats": page,
            })
        })
        .collect();
    Ok(json!({"groups": groups}))
}

/// The projects: every chat's folder, except the home and scratch folders.
fn list_projects(engine: &Engine) -> Value {
    let mut folders: Vec<PathBuf> = engine
        .chats()
        .into_iter()
        .map(|chat| chat.cwd)
        .filter(|cwd| engine.config().is_project(cwd))
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
    let read = if args.known.as_deref() == Some(version.as_str()) {
        None
    } else {
        let runs = match engine.run_counts(&args.chat).await {
            Ok(runs) => turn_changes_of(&view.page.messages, &runs),
            // The chat still shows; it only loses its changes buttons.
            Err(e) => {
                eprintln!("reading chat {}'s run changes: {e:#}", args.chat);
                Vec::new()
            }
        };
        Some(json!({
            "total": view.page.total,
            "olderRemaining": view.page.older(),
            "pendingInput": view.question.as_ref().map(pending_input),
            "messages": view.page.messages,
            "turnChanges": runs,
        }))
    };
    Ok(json!({
        "chat": summary(&view.chat, engine.config()),
        "version": version,
        "read": read,
    }))
}

/// The saved runs among `messages` that changed files, as
/// `{turn, after, files, additions, deletions}`. `turn` names the run for
/// `turn_changes`, and `after` is its last message, which the button
/// follows. A run lasts until the next one starts, so steering messages
/// stay in it.
fn turn_changes_of(messages: &[Message], runs: &BTreeMap<i64, Counts>) -> Vec<Value> {
    let starts = |m: &Message| m.role == Role::User && runs.contains_key(&m.created_at);
    let mut out = Vec::new();
    for (i, message) in messages.iter().enumerate() {
        let Some(counts) = runs.get(&message.created_at).filter(|_| starts(message)) else {
            continue;
        };
        if counts.files == 0 {
            continue;
        }
        let end = messages[i + 1..]
            .iter()
            .position(starts)
            .map_or(messages.len(), |n| i + 1 + n);
        out.push(json!({
            "turn": message.created_at.to_string(),
            "after": messages[end - 1].id,
            "files": counts.files,
            "additions": counts.additions,
            "deletions": counts.deletions,
        }));
    }
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnArgs {
    chat: String,
    /// A `turn` from `chat_view`'s `turnChanges`.
    turn: String,
}

/// What one run changed: `{patch, files, additions, deletions, truncated}`.
async fn turn_changes(engine: &Engine, args: TurnArgs) -> Result<Value> {
    find(engine, &args.chat)?;
    let key = args
        .turn
        .parse()
        .with_context(|| format!("no run {}", args.turn))?;
    match engine.run_changes(&args.chat, key).await? {
        Some(diff) => Ok(json!(diff)),
        None => bail!("this run's changes weren't kept"),
    }
}

/// The one harness, as the phone's agent picker lists harnesses.
fn list_harnesses() -> Value {
    json!({"harnesses": [{
        "id": "pi",
        "name": "Pi",
        "available": true,
        "installed": true,
        "steersMidTurn": true,
        // For models pi doesn't list, so their own levels are unknown.
        "reasoningLevels": ["off", "minimal", "low", "medium", "high"],
    }]})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListModelsArgs {
    harness: String,
}

/// The models pi can use, with the thinking levels each supports.
async fn list_models(engine: &Engine, args: ListModelsArgs) -> Result<Value> {
    pi_only(Some(&args.harness))?;
    let models = engine.models().await?;
    let models: Vec<Value> = models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "label": model.name,
                "description": null,
                "reasoningLevels": model.levels,
            })
        })
        .collect();
    Ok(json!({"harness": "pi", "models": models}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetModelArgs {
    chat: String,
    harness: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
}

/// Change a chat's model or thinking level from its next answer, and
/// return its composer settings then.
async fn set_chat_model(engine: &Engine, args: SetModelArgs) -> Result<Value> {
    pi_only(args.harness.as_deref())?;
    find(engine, &args.chat)?;
    let (model, reasoning) = (args.model.as_deref(), args.reasoning.as_deref());
    let composer = engine.set_model(&args.chat, model, reasoning).await?;
    Ok(settings(&composer))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposerInfoArgs {
    chat: Option<String>,
    /// For a new chat, the project it would start in.
    project: Option<PathBuf>,
}

/// What the composer shows under the text: the checkout and branch, the
/// context use, and the model settings. A new chat has only the first.
async fn composer_info(engine: &Engine, args: ComposerInfoArgs) -> Result<Value> {
    let (cwd, composer) = match &args.chat {
        Some(id) => {
            let chat = find(engine, id)?;
            (Some(chat.cwd), Some(engine.composer(id).await?))
        }
        None => (args.project, None),
    };
    let checkout = match &cwd {
        Some(cwd) => repo::checkout(cwd).await,
        None => None,
    };
    let mut answer = composer.as_ref().map_or_else(|| json!({}), settings);
    answer["harness"] = json!("pi");
    answer["checkout"] = json!(checkout.as_ref().map(|checkout| {
        if checkout.worktree {
            "worktree"
        } else {
            "local"
        }
    }));
    answer["branch"] = json!(checkout.and_then(|checkout| checkout.branch));
    Ok(answer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListSkillsArgs {
    chat: Option<String>,
    harness: Option<String>,
    /// For a new chat, the project it would start in.
    project: Option<PathBuf>,
}

/// The skills a chat, or a new chat in a project, can run. A skill's `ref`
/// is the command the composer puts where its `$name` was typed.
async fn list_skills(engine: &Engine, args: ListSkillsArgs) -> Result<Value> {
    pi_only(args.harness.as_deref())?;
    let skills = match &args.chat {
        Some(id) => engine.skills(id).await?,
        None => engine.skills_in(args.project.as_deref()).await?,
    };
    let skills: Vec<Value> = skills
        .iter()
        .map(|skill| {
            json!({"name": skill.name, "description": skill.description, "ref": skill.command()})
        })
        .collect();
    Ok(json!({"skills": skills}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolishArgs {
    text: String,
}

/// What dictation heard, cleaned up, in the speaker's words.
async fn polish_dictation(engine: &Engine, args: PolishArgs) -> Result<Value> {
    Ok(json!({"text": engine.polish(&args.text).await?}))
}

/// A chat's model settings and context use, as the composer reads them.
fn settings(composer: &Composer) -> Value {
    json!({
        "model": composer.model,
        "reasoning": composer.thinking,
        "contextUsage": {
            "tokens": composer.context_tokens,
            "window": composer.context_window,
        },
    })
}

/// Fail unless `harness` is pi or unset: it's the only one Ludolph runs.
fn pi_only(harness: Option<&str>) -> Result<()> {
    match harness {
        None | Some("pi") => Ok(()),
        Some(other) => bail!("Ludolph runs only pi, not {other}"),
    }
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
        "title": before.title,
        "sent": sent(delivery, before.status),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateChatArgs {
    /// The project's folder; without one, the chat gets a scratch folder.
    project: Option<PathBuf>,
    prompt: Option<String>,
    harness: Option<String>,
    /// `provider/model`; without one, pi's default.
    model: Option<String>,
    reasoning: Option<String>,
    /// The branch the phone saw. Unused: the chat runs in the project's
    /// checkout as it is.
    #[serde(rename = "branch")]
    _branch: Option<String>,
}

/// Create a chat, in a project or a scratch folder, and send it the first
/// prompt. If the chat was created but the prompt didn't go, `sendError`
/// says why: retrying the whole call would create a second chat.
async fn create_chat(engine: &Engine, args: CreateChatArgs) -> Result<Value> {
    pi_only(args.harness.as_deref())?;
    let cwd = match &args.project {
        Some(project) => project.clone(),
        None => {
            let day = chrono::Local::now().format("%Y-%m-%d");
            let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
            let cwd = engine.config().scratch_dir().join(format!("{day}-{tag}"));
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
    if args.model.is_some() || args.reasoning.is_some() {
        let (model, reasoning) = (args.model.as_deref(), args.reasoning.as_deref());
        // The prompt isn't sent with the wrong model, and the phone keeps it.
        if let Err(e) = engine.set_model(&id, model, reasoning).await {
            answer["sendError"] = json!(format!("setting the model: {e:#}"));
            return Ok(answer);
        }
    }
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

/// The question the phone and the Companion show for a dialog: one question,
/// in the shape of Claude Code's `AskUserQuestion`.
fn pending_input(question: &Question) -> Value {
    let (header, text) = match &question.message {
        Some(message) => (Some(question.title.as_str()), message.as_str()),
        None => (None, question.title.as_str()),
    };
    let options: Vec<&str> = match question.dialog {
        Dialog::Select => question.options.iter().map(String::as_str).collect(),
        Dialog::Confirm => CONFIRM.to_vec(),
        Dialog::Input | Dialog::Editor => Vec::new(),
    };
    json!({
        "requestId": question.id,
        "questions": [{
            "id": ANSWER,
            "header": header,
            "question": text,
            "options": options,
            "multiSelect": false,
            "multiline": question.dialog == Dialog::Editor,
            "prefill": question.prefill,
        }],
    })
}

/// The one question's id in `pending_input`.
const ANSWER: &str = "answer";

/// A `confirm`'s options, yes first.
const CONFIRM: [&str; 2] = ["Yes", "No"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondArgs {
    chat: String,
    /// The question's `requestId`; the open one when it's left out.
    request_id: Option<String>,
    answers: Vec<Answer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    #[serde(rename = "question_id")]
    _question_id: String,
    /// The picked options, then any typed text.
    labels: Vec<String>,
}

/// Answer the dialog a chat waits on.
async fn respond_to_input(engine: &Engine, args: RespondArgs) -> Result<Value> {
    let question = engine
        .question(&args.chat)
        .filter(|q| args.request_id.as_ref().is_none_or(|id| *id == q.id))
        .context("this chat has no such open question")?;
    let labels = args
        .answers
        .into_iter()
        .next()
        .map(|a| a.labels)
        .unwrap_or_default();
    let Some(first) = labels.first() else {
        bail!("answers is empty");
    };
    let reply = match question.dialog {
        Dialog::Confirm => match CONFIRM.iter().position(|label| label == first) {
            Some(at) => Reply::Confirmed(at == 0),
            None => bail!("answer Yes or No"),
        },
        Dialog::Select => Reply::Value(first.clone()),
        Dialog::Input | Dialog::Editor => Reply::Value(labels.join("\n")),
    };
    engine.respond(&args.chat, &question.id, reply).await?;
    Ok(json!({"chatId": args.chat, "requestId": question.id}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveArgs {
    chat: String,
    /// `false` unarchives.
    archived: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportArgs {
    chats: Vec<Imported>,
}

/// One chat's title and archive state, from another app.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Imported {
    chat: String,
    title: String,
    archived: bool,
}

fn import_chats(engine: &Engine, args: ImportArgs) -> Result<Value> {
    let chats: Vec<_> = args
        .chats
        .into_iter()
        .map(|c| (c.chat, c.title, c.archived))
        .collect();
    let found = engine.import(&chats)?;
    Ok(json!({"imported": found, "unknown": chats.len() - found}))
}

fn archive_chat(engine: &Engine, args: ArchiveArgs) -> Result<Value> {
    let archived = args.archived.unwrap_or(true);
    engine.archive(&args.chat, archived)?;
    let title = find(engine, &args.chat)?.title;
    Ok(json!({"chatId": args.chat, "title": title, "archived": archived}))
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
fn summary(chat: &ChatInfo, config: &Config) -> Value {
    let summary = chat.summary.as_ref();
    json!({
        "id": chat.id,
        "title": chat.title,
        "project": config.is_project(&chat.cwd).then(|| project(&chat.cwd)),
        "cwd": chat.cwd,
        "harness": "pi",
        "model": summary.and_then(|s| s.model.as_deref()),
        "reasoning": summary.and_then(|s| s.thinking.as_deref()),
        "archived": chat.archived,
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
        Status::AwaitingInput => "awaitingInput",
        Status::Failed => "errored",
        Status::Interrupted => "interrupted",
    }
}
