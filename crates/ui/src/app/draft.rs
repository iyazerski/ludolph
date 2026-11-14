//! A new chat until its first message creates it: its project, model and
//! thinking level, and pi's catalog of models to pick from.

use serde_json::json;

use super::attach::Payload;
use super::{Page, Pending, State, Target};
use crate::data::{Created, Model, Models, Picked, Project, Projects, levels_of};
use crate::labels::short_model;
use crate::prefs::{NewChat, ProjectPick};

/// The picks of a chat that isn't created yet.
pub struct Draft {
    pub projects: Vec<Project>,
    pub project: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub load: Load,
    pub creating: bool,
    saved_project: ProjectPick,
}

/// Where loading a draft's projects is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Load {
    Loading,
    Ready,
    /// A load failed; it runs again when the engine comes back.
    Failed,
}

/// The model and thinking level the composer shows.
#[derive(Clone, Copy, Default)]
pub struct Config<'a> {
    pub model: Option<&'a str>,
    pub reasoning: Option<&'a str>,
}

/// One pick in the model sheet.
pub enum Pick {
    Model(String),
    Reasoning(String),
}

#[derive(Default)]
pub enum Catalog {
    /// Not asked for yet.
    #[default]
    Unloaded,
    Loading,
    Ready(Models),
    /// Loading failed; the next [`State::load_catalog`] call tries again.
    Failed(String),
}

impl Draft {
    /// A draft with the last new chat's picks, kept if they're still valid once loaded.
    pub(super) fn new(saved: NewChat) -> Self {
        let NewChat {
            project,
            model,
            reasoning,
        } = saved;
        Self {
            projects: Vec::new(),
            project: match &project {
                ProjectPick::Project(id) => Some(id.clone()),
                ProjectPick::Recent | ProjectPick::NoProject => None,
            },
            model,
            reasoning,
            load: Load::Loading,
            creating: false,
            saved_project: project,
        }
    }
}

impl Catalog {
    pub fn ready(&self) -> Option<&[Model]> {
        match self {
            Self::Ready(got) => Some(&got.models),
            Self::Unloaded | Self::Loading | Self::Failed(_) => None,
        }
    }

    /// Model `id`'s name, as pi labels it, or a short form of its id.
    pub fn label(&self, id: &str) -> String {
        self.ready()
            .and_then(|models| models.iter().find(|m| m.id == id))
            .and_then(|m| m.label.clone())
            .unwrap_or_else(|| short_model(id).to_owned())
    }

    /// The model pi starts a chat on, once loaded.
    pub fn default_model(&self) -> Option<&str> {
        match self {
            Self::Ready(got) => got.default.as_deref(),
            Self::Unloaded | Self::Loading | Self::Failed(_) => None,
        }
    }
}

impl State {
    /// Load a new chat's projects, keeping a valid pick, and the models.
    pub(super) fn load_draft(&mut self) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        if let Some(draft) = page.draft_mut() {
            draft.load = Load::Loading;
        }
        self.load_catalog();
        self.api.call(
            "list_projects",
            json!({}),
            move |s, result: Result<Projects, String>, _| {
                let recent = s
                    .list
                    .chats
                    .as_ref()
                    .and_then(|c| c.first()?.project.as_ref())
                    .map(|p| p.id.clone());
                let Some(draft) = s.page_at(nav).and_then(|p| p.draft_mut()) else {
                    return;
                };
                match result {
                    Ok(mut got) => {
                        got.projects.sort_by(|a, b| a.name.cmp(&b.name));
                        draft.projects = got.projects;
                        if draft.saved_project == ProjectPick::Recent {
                            draft.project = recent;
                        }
                        if !draft
                            .projects
                            .iter()
                            .any(|p| Some(&p.id) == draft.project.as_ref())
                        {
                            draft.project = None;
                        }
                        draft.load = Load::Ready;
                        s.load_info();
                        s.load_skills();
                    }
                    Err(e) => {
                        draft.load = Load::Failed;
                        s.fail("Couldn't load the projects", e);
                    }
                }
            },
        );
    }

    /// Create the chat with the first message, then show it in place of the draft.
    pub(super) fn create(&mut self, text: String, images: Vec<Payload>) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let typed = page.composer.text.trim().to_owned();
        let thumbs: Vec<_> = page.attached.iter().map(|a| a.thumb.clone()).collect();
        let Some(draft) = page.draft_mut() else {
            return;
        };
        draft.creating = true;
        let mut args = json!({ "prompt": text });
        for (key, value) in [
            ("project", &draft.project),
            ("model", &draft.model),
            ("reasoning", &draft.reasoning),
        ] {
            if let Some(value) = value {
                args[key] = json!(value);
            }
        }
        if !images.is_empty() {
            args["images"] = json!(images);
        }
        let pick = NewChat {
            project: ProjectPick::from(draft.project.clone()),
            model: draft.model.clone(),
            reasoning: draft.reasoning.clone(),
        };
        let at = chrono::Utc::now().timestamp_millis();
        self.api.call(
            "create_chat",
            args,
            move |s, result: Result<Created, String>, _| {
                let Some(page) = s.page_at(nav) else { return };
                let made = match result {
                    Ok(made) => made,
                    Err(e) => {
                        if let Some(draft) = page.draft_mut() {
                            draft.creating = false;
                        }
                        s.fail("Couldn't start the chat", e);
                        return;
                    }
                };
                // The chat exists, so there's no text to keep for the next new chat.
                let left = std::mem::take(&mut page.composer.text);
                // Also resets the caret, history, and marks.
                page.composer.clear();
                page.attached.clear();
                s.prefs.borrow_mut().new_chat = pick;
                s.save_prefs();
                if let Some(why) = made.send_error {
                    // Open it with the message back in its composer.
                    s.drafts.insert(made.chat_id.clone(), left);
                    s.open_chat(made.chat_id);
                    s.fail("Chat started, but the message wasn't sent", why);
                    return;
                }
                s.open_chat(made.chat_id);
                if let Some(chat) = s.chat_mut().and_then(|p| p.existing_mut()) {
                    chat.pending.push(Pending {
                        text: typed,
                        images: thumbs,
                        at,
                    });
                }
            },
        );
    }

    /// Load pi's models unless they're loaded or loading.
    pub fn load_catalog(&mut self) {
        if matches!(self.catalog, Catalog::Loading | Catalog::Ready(_)) {
            return;
        }
        self.catalog = Catalog::Loading;
        self.api.call(
            "list_models",
            json!({}),
            |s, result: Result<Models, String>, _| {
                s.catalog = match result {
                    Ok(got) => Catalog::Ready(got),
                    Err(e) => {
                        eprintln!("loading the models: {e}");
                        Catalog::Failed(e)
                    }
                };
                s.fit_draft();
            },
        );
    }

    /// The model and thinking level the composer shows: a new chat's
    /// picks, else the chat's.
    pub fn config(&self) -> Config<'_> {
        let Some(page) = self.chat() else {
            return Config::default();
        };
        let chat = match &page.target {
            Target::New(d) => {
                return Config {
                    model: d.model.as_deref(),
                    reasoning: d.reasoning.as_deref(),
                };
            }
            Target::Existing(chat) => chat.chat.as_ref(),
        };
        match &page.info {
            Some(info) => Config {
                model: info.model.as_deref(),
                reasoning: info.reasoning.as_deref(),
            },
            None => Config {
                model: chat.and_then(|c| c.model.as_deref()),
                reasoning: chat.and_then(|c| c.reasoning.as_deref()),
            },
        }
    }

    /// Pick a model or thinking level for the next turns.
    pub fn pick(&mut self, pick: Pick) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let id = match &page.target {
            Target::New(_) => return self.pick_draft(pick),
            Target::Existing(chat) => chat.id.clone(),
        };
        let args = match pick {
            Pick::Model(model) => json!({ "chat": id, "model": model, "reasoning": null }),
            Pick::Reasoning(level) => json!({ "chat": id, "model": null, "reasoning": level }),
        };
        self.api.call(
            "set_chat_model",
            args,
            move |s, result: Result<Picked, String>, _| match result {
                Ok(saved) => {
                    if let Some(info) = s.page_at(nav).and_then(|p| p.info.as_mut()) {
                        info.model = saved.model;
                        info.reasoning = saved.reasoning;
                    }
                }
                Err(e) => s.fail("Couldn't change the model", e),
            },
        );
    }

    fn pick_draft(&mut self, pick: Pick) {
        let Some(draft) = self.chat_mut().and_then(|p| p.draft_mut()) else {
            return;
        };
        match pick {
            Pick::Model(model) => draft.model = Some(model),
            Pick::Reasoning(level) => draft.reasoning = Some(level),
        }
        self.fit_draft();
    }

    /// Keep a new chat's model among pi's and its thinking level on the model's.
    fn fit_draft(&mut self) {
        let Page::Chat(page) = &mut self.page else {
            return;
        };
        let Some(draft) = page.draft_mut() else {
            return;
        };
        let Some(models) = self.catalog.ready() else {
            return;
        };
        if draft
            .model
            .as_ref()
            .is_some_and(|m| !models.iter().any(|x| &x.id == m))
        {
            draft.model = None;
        }
        let levels = levels_of(models, draft.model.as_deref().unwrap_or_default());
        if draft
            .reasoning
            .as_ref()
            .is_some_and(|r| !levels.contains(r))
        {
            draft.reasoning = None;
        }
    }

    pub fn pick_project(&mut self, project: Option<String>) {
        let Some(page) = self.chat_mut() else { return };
        let Some(draft) = page.draft_mut() else {
            return;
        };
        if draft.project == project {
            return;
        }
        draft.project = project;
        page.info = None;
        self.load_info();
        self.reload_skills();
    }
}
