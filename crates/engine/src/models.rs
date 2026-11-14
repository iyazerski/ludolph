//! The models pi can use, for the composer's picker.
//!
//! pi lists them only over RPC (`get_available_models`), so the catalog asks
//! a short-lived pi without a session, and keeps the answer for a while: the
//! list changes only when someone edits pi's settings or logs in. The same pi
//! says which model it starts on (`get_state`): its default.

mod sources;

pub(crate) use sources::Sources;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use crate::pi::Pi;

/// How long a fetched list is used before pi is asked again.
const FRESH: Duration = Duration::from_mins(10);

/// pi's thinking levels, in order (`getSupportedThinkingLevels` in pi-ai).
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// One model pi can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// `provider/model`, which [`crate::engine::Engine::set_model`] takes.
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    /// The thinking levels it supports, just `off` if it doesn't reason.
    pub levels: Vec<String>,
}

impl Model {
    /// Read one of pi's model objects (`docs/rpc-commands.md`), if it has
    /// an id and a provider.
    pub fn from_pi(model: &Value) -> Option<Self> {
        let provider = model["provider"].as_str()?;
        let id = model["id"].as_str()?;
        Some(Self {
            id: format!("{provider}/{id}"),
            name: model["name"].as_str().unwrap_or(id).to_owned(),
            context_window: model["contextWindow"].as_u64().filter(|&size| size > 0),
            levels: levels(model),
        })
    }
}

/// The thinking levels `model` supports, as pi works them out: none past
/// `off` without reasoning, and `xhigh` and `max` only when its level map
/// names them.
fn levels(model: &Value) -> Vec<String> {
    if model["reasoning"].as_bool() != Some(true) {
        return vec!["off".to_owned()];
    }
    let map = &model["thinkingLevelMap"];
    LEVELS
        .into_iter()
        .filter(|&level| match map.get(level) {
            Some(Value::Null) => false,
            Some(_) => true,
            None => !matches!(level, "xhigh" | "max"),
        })
        .map(str::to_owned)
        .collect()
}

/// The models pi can use, fetched when first needed and again once stale.
#[derive(Debug)]
pub struct Catalog {
    program: PathBuf,
    /// Where the fetching pi runs: pi reads project settings from its folder,
    /// so this is one without any.
    cwd: PathBuf,
    grace: Duration,
    /// Held while fetching, so callers at the same time share one pi.
    cached: Mutex<Option<(Instant, Sources, Available)>>,
    files: [PathBuf; 3],
    observed: StdMutex<Sources>,
    changes: watch::Sender<u64>,
}

impl Catalog {
    /// Track pi's configuration in `agent` and ask `program` in `cwd` for its models.
    pub fn new(program: PathBuf, cwd: PathBuf, agent: &Path, grace: Duration) -> Result<Self> {
        let files = ["auth.json", "models.json", "settings.json"].map(|name| agent.join(name));
        let observed = Sources::read(&files)?;
        Ok(Self {
            program,
            cwd,
            grace,
            cached: Mutex::new(None),
            files,
            observed: StdMutex::new(observed),
            changes: watch::channel(0).0,
        })
    }

    /// Follow changes to pi's credentials and model configuration.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Read the configuration revision without reading any credentials.
    pub(crate) fn sources(&self) -> Result<Sources> {
        let sources = Sources::read(&self.files)?;
        let mut observed = self.observed.lock().expect("lock poisoned");
        if *observed != sources {
            *observed = sources.clone();
            self.changes.send_modify(|revision| *revision += 1);
        }
        Ok(sources)
    }

    /// Whether a reported filesystem path can change the model list.
    pub(crate) fn watches(&self, path: &Path) -> bool {
        self.files.iter().any(|file| file == path)
    }

    /// Read the model list and defaults from one pi snapshot.
    ///
    /// # Errors
    ///
    /// Fail if pi cannot start or list its models.
    pub async fn available(&self) -> Result<Available> {
        let mut cached = self.cached.lock().await;
        let sources = self.sources()?;
        if let Some((at, saved, fetched)) = &*cached
            && *saved == sources
            && at.elapsed() < FRESH
        {
            return Ok(fetched.clone());
        }
        let fetched = self.fetch().await?;
        *cached = Some((Instant::now(), sources, fetched.clone()));
        Ok(fetched)
    }

    /// The model `id`, if pi lists it.
    ///
    /// # Errors
    ///
    /// Fails if the models can't be fetched.
    pub async fn find(&self, id: &str) -> Result<Option<Model>> {
        let available = self.available().await?;
        Ok(available
            .models
            .iter()
            .find(|model| model.id == id)
            .cloned())
    }

    /// Ask a new pi for its models and the one it starts on.
    async fn fetch(&self) -> Result<Available> {
        let commands = vec![
            json!({"type": "get_available_models"}),
            json!({"type": "get_state"}),
        ];
        let answers = Pi::ask_all(&self.program, &self.cwd, self.grace, commands)
            .await
            .context("listing pi's models")?;
        let models = answers[0]["models"]
            .as_array()
            .context("pi listed no models")?;
        let model = &answers[1]["model"];
        let default = model["provider"]
            .as_str()
            .zip(model["id"].as_str())
            .map(|(provider, id)| format!("{provider}/{id}"));
        let reasoning = answers[1]["thinkingLevel"]
            .as_str()
            .unwrap_or("medium")
            .to_owned();
        Ok(Available {
            reasoning,
            models: Arc::new(models.iter().filter_map(Model::from_pi).collect()),
            default,
        })
    }
}

/// What one pi said: its models, and the one it starts on.
#[derive(Debug, Clone)]
pub struct Available {
    /// The default thinking level, Medium if pi omitted it.
    pub reasoning: String,
    /// The models available to this sign-in.
    pub models: Arc<Vec<Model>>,
    /// The model pi starts on.
    pub default: Option<String>,
}
