//! The tools the app and the phone call, by name, with JSON arguments and
//! answers. The settings and phone link tools are the app's alone.

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
use crate::models::Model;
use crate::repo;
use crate::transcript::{Image, LATEST, Message, Part, Role, Window};
use crate::turns::Counts;

/// Chats `list_chats` returns without a `limit`, and at most.
const DEFAULT_CHATS: usize = 50;
const MAX_CHATS: usize = 500;
/// Chats `list_chat_groups` returns per group without a `limit`.
const DEFAULT_GROUP_CHATS: usize = 10;
/// The most messages `chat_view` returns from the end, or adds before its
/// anchor, in one call.
const MAX_MESSAGES: usize = 500;
/// The most images one message carries.
const MAX_IMAGES: usize = 8;
/// The image types models take.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A tool, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ListChats,
    ListChatGroups,
    ListProjects,
    ChatView,
    ToolOutput,
    ChatImage,
    SendMessage,
    CreateChat,
    InterruptChat,
    TakeOverChat,
    RespondToInput,
    RenameChat,
    ArchiveChat,
    TurnChanges,
    ListModels,
    SetChatModel,
    ComposerInfo,
    ListSkills,
    PolishDictation,
    GetSettings,
    SetSettings,
    GetPhoneLink,
    LinkPhone,
    UnlinkPhone,
}

impl Tool {
    /// Whether the phone may call it. The phone can't change settings or
    /// the link, so a relay can't, say, point dictation at a model of its
    /// choosing, or the Mac at another relay.
    pub fn for_phone(self) -> bool {
        !matches!(
            self,
            Self::GetSettings
                | Self::SetSettings
                | Self::GetPhoneLink
                | Self::LinkPhone
                | Self::UnlinkPhone
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
            "tool_output" => Self::ToolOutput,
            "chat_image" => Self::ChatImage,
            "send_message" => Self::SendMessage,
            "create_chat" => Self::CreateChat,
            "interrupt_chat" => Self::InterruptChat,
            "take_over_chat" => Self::TakeOverChat,
            "respond_to_input" => Self::RespondToInput,
            "rename_chat" => Self::RenameChat,
            "archive_chat" => Self::ArchiveChat,
            "turn_changes" => Self::TurnChanges,
            "list_models" => Self::ListModels,
            "set_chat_model" => Self::SetChatModel,
            "composer_info" => Self::ComposerInfo,
            "list_skills" => Self::ListSkills,
            "polish_dictation" => Self::PolishDictation,
            "get_settings" => Self::GetSettings,
            "set_settings" => Self::SetSettings,
            "get_phone_link" => Self::GetPhoneLink,
            "link_phone" => Self::LinkPhone,
            "unlink_phone" => Self::UnlinkPhone,
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
        Tool::ToolOutput => {
            let args: ToolArgs = parse(args)?;
            Ok(serde_json::to_value(
                engine.tool_output(&args.chat, &args.call).await?,
            )?)
        }
        Tool::ChatImage => {
            let args: ImageArgs = parse(args)?;
            let image = engine.image(&args.chat, &args.message, args.index).await?;
            Ok(serde_json::to_value(image)?)
        }
        Tool::SendMessage => send_message(engine, parse(args)?).await,
        Tool::CreateChat => create_chat(engine, parse(args)?).await,
        Tool::InterruptChat => interrupt_chat(engine, parse(args)?).await,
        Tool::TakeOverChat => take_over_chat(engine, parse(args)?).await,
        Tool::RespondToInput => respond_to_input(engine, parse(args)?).await,
        Tool::RenameChat => rename_chat(engine, parse(args)?).await,
        Tool::ArchiveChat => archive_chat(engine, parse(args)?),
        Tool::TurnChanges => turn_changes(engine, parse(args)?).await,
        Tool::ListModels => list_models(engine).await,
        Tool::SetChatModel => set_chat_model(engine, parse(args)?).await,
        Tool::ComposerInfo => composer_info(engine, parse(args)?).await,
        Tool::ListSkills => list_skills(engine, parse(args)?).await,
        Tool::PolishDictation => polish_dictation(engine, parse(args)?).await,
        Tool::GetSettings => Ok(serde_json::to_value(engine.settings())?),
        // All of them at once: a field left out takes its default.
        Tool::SetSettings => Ok(serde_json::to_value(
            engine.set_settings(parse(args)?).await?,
        )?),
        Tool::GetPhoneLink => Ok(serde_json::to_value(engine.phone())?),
        Tool::LinkPhone => {
            let args: LinkArgs = parse(args)?;
            Ok(serde_json::to_value(engine.link_phone(&args.code).await?)?)
        }
        Tool::UnlinkPhone => {
            engine.unlink_phone()?;
            Ok(Value::Null)
        }
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T> {
    // A tool called without arguments gets none.
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).context("invalid arguments")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkArgs {
    /// From the phone's "Add a Mac".
    code: String,
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
    /// How many of the latest messages, without `from`.
    limit: Option<usize>,
    /// The message to show from, to the end: the first one shown, so the
    /// page keeps it as new messages come.
    from: Option<String>,
    /// With `from`, how many more to add before it, to load earlier ones.
    #[serde(default)]
    earlier: usize,
    /// Include the model's thinking, often most of a page's bytes, which
    /// the phone doesn't show.
    #[serde(default)]
    thinking: bool,
    /// The `version` the caller shows.
    known: Option<String>,
}

/// A chat and its latest messages, or those from a message on. `read` is
/// `null` when the caller already shows this `version`.
async fn chat_view(engine: &Engine, args: ViewArgs) -> Result<Value> {
    let window = match args.from {
        Some(id) => Window::From {
            id,
            earlier: args.earlier.min(MAX_MESSAGES),
        },
        None => Window::Latest(args.limit.unwrap_or(LATEST).clamp(1, MAX_MESSAGES)),
    };
    let shape = match &window {
        Window::Latest(n) => format!("latest {n}"),
        Window::From { id, earlier } => format!("from {id} {earlier}"),
    };
    let mut view = engine.view(&args.chat, window).await?;
    if !args.thinking {
        for message in &mut view.page.messages {
            message
                .parts
                .retain(|part| !matches!(part, Part::Thinking { .. }));
        }
    }
    let version = format!(
        "{}|{}|{}|{shape}",
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
            "run": view.run,
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
pub(crate) fn turn_changes_of(messages: &[Message], runs: &BTreeMap<i64, Counts>) -> Vec<Value> {
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
struct ToolArgs {
    chat: String,
    /// A tool part's `id`.
    call: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageArgs {
    chat: String,
    /// The message's `id`.
    message: String,
    /// An image part's `index`.
    index: usize,
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

/// The models pi can use, with the thinking levels each supports.
async fn list_models(engine: &Engine) -> Result<Value> {
    let models = engine.models().await?;
    let models: Vec<Value> = models.iter().map(model).collect();
    Ok(json!({"models": models}))
}

fn model(model: &Model) -> Value {
    json!({"id": model.id, "label": model.name, "reasoningLevels": model.levels})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetModelArgs {
    chat: String,
    model: Option<String>,
    reasoning: Option<String>,
}

/// Change a chat's model or thinking level from its next answer, and
/// return its composer settings then.
async fn set_chat_model(engine: &Engine, args: SetModelArgs) -> Result<Value> {
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
    /// For a new chat, the project it would start in.
    project: Option<PathBuf>,
}

/// The skills a chat, or a new chat in a project, can run. A skill's `ref`
/// is the command the composer puts where its `$name` was typed.
async fn list_skills(engine: &Engine, args: ListSkillsArgs) -> Result<Value> {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    chat: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    images: Vec<Image>,
}

/// Send text, images or both to a chat: start a run, or steer the one
/// that's going.
async fn send_message(engine: &Engine, args: SendArgs) -> Result<Value> {
    let text = args.text.trim();
    let images = checked(args.images)?;
    if text.is_empty() && images.is_empty() {
        bail!("the message is empty");
    }
    find(engine, &args.chat)?;
    let delivery = engine.send(&args.chat, text, images).await?;
    Ok(json!({"chatId": args.chat, "delivery": delivery_name(delivery)}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateChatArgs {
    /// The project's folder; without one, the chat gets a scratch folder.
    project: Option<PathBuf>,
    prompt: Option<String>,
    /// Sent with the prompt.
    #[serde(default)]
    images: Vec<Image>,
    /// `provider/model`; without one, pi's default.
    model: Option<String>,
    reasoning: Option<String>,
}

/// Create a chat, in a project or a scratch folder, and send it the first
/// prompt. If the chat was created but the prompt didn't go, `sendError`
/// says why: retrying the whole call would create a second chat.
async fn create_chat(engine: &Engine, args: CreateChatArgs) -> Result<Value> {
    let images = checked(args.images)?;
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
        "project": args.project.as_deref().map(project),
    });
    if args.model.is_some() || args.reasoning.is_some() {
        let (model, reasoning) = (args.model.as_deref(), args.reasoning.as_deref());
        // The prompt isn't sent with the wrong model, and the phone keeps it.
        if let Err(e) = engine.set_model(&id, model, reasoning).await {
            answer["sendError"] = json!(format!("setting the model: {e:#}"));
            return Ok(answer);
        }
    }
    let prompt = args.prompt.as_deref().map_or("", str::trim);
    if !prompt.is_empty() || !images.is_empty() {
        match engine.send(&id, prompt, images).await {
            Ok(delivery) => answer["delivery"] = json!(delivery_name(delivery)),
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

/// An extension's open dialog, as pi asks it (`docs/rpc-extension-ui.md`).
pub(crate) fn pending_input(question: &Question) -> Value {
    let dialog = match question.dialog {
        Dialog::Select => "select",
        Dialog::Confirm => "confirm",
        Dialog::Input => "input",
        Dialog::Editor => "editor",
    };
    json!({
        "id": question.id,
        "dialog": dialog,
        "title": question.title,
        "message": question.message,
        "options": question.options,
        "prefill": question.prefill,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondArgs {
    chat: String,
    /// The dialog's `id`, so an answer can't land on a newer one.
    id: String,
    /// A `select`'s option, or the typed text.
    value: Option<String>,
    /// A `confirm`'s answer.
    confirmed: Option<bool>,
}

/// Answer the dialog a chat waits on.
async fn respond_to_input(engine: &Engine, args: RespondArgs) -> Result<Value> {
    let question = engine
        .question(&args.chat)
        .filter(|q| q.id == args.id)
        .context("this chat has no such open question")?;
    let reply = match (question.dialog, args.value, args.confirmed) {
        (Dialog::Confirm, None, Some(yes)) => Reply::Confirmed(yes),
        (Dialog::Confirm, ..) => bail!("a confirm takes `confirmed`"),
        (Dialog::Select, Some(value), None) if question.options.contains(&value) => {
            Reply::Value(value)
        }
        (Dialog::Select, ..) => bail!("pick one of the options"),
        (Dialog::Input | Dialog::Editor, Some(value), None) => Reply::Value(value),
        (Dialog::Input | Dialog::Editor, ..) => bail!("this dialog takes a `value`"),
    };
    engine.respond(&args.chat, &question.id, reply).await?;
    Ok(json!({"chatId": args.chat}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameArgs {
    chat: String,
    title: String,
}

async fn rename_chat(engine: &Engine, args: RenameArgs) -> Result<Value> {
    engine.rename(&args.chat, &args.title).await?;
    Ok(json!({"chatId": args.chat, "title": args.title.trim()}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveArgs {
    chat: String,
    /// `false` unarchives.
    archived: Option<bool>,
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
pub(crate) fn summary(chat: &ChatInfo, config: &Config) -> Value {
    let summary = chat.summary.as_ref();
    json!({
        "id": chat.id,
        "title": chat.title,
        "project": config.is_project(&chat.cwd).then(|| project(&chat.cwd)),
        "cwd": chat.cwd,
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

/// `images`, if a model can take them.
fn checked(images: Vec<Image>) -> Result<Vec<Image>> {
    if images.len() > MAX_IMAGES {
        bail!("a message takes at most {MAX_IMAGES} images");
    }
    if let Some(image) = images
        .iter()
        .find(|image| !IMAGE_TYPES.contains(&image.mime_type.as_str()))
    {
        bail!("models don't take {} images", image.mime_type);
    }
    Ok(images)
}

/// How a message went: it started a run, steers the one going, or was a
/// command pi handled itself.
fn delivery_name(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Started => "run",
        Delivery::Queued => "steer",
        Delivery::Handled => "handled",
    }
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
