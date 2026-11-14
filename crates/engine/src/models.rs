//! The models pi can use, for the composer's picker.
//!
//! pi lists them only over RPC (`get_available_models`), so the catalog asks
//! a short-lived pi without a session, and keeps the answer for a while: the
//! list changes only when someone edits pi's settings or logs in. The same pi
//! says which model it starts on (`get_state`): its default.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::Mutex;
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
    cached: Mutex<Option<(Instant, Fetched)>>,
}

impl Catalog {
    /// A catalog that runs `program` in `cwd`, giving it `grace` to exit.
    pub fn new(program: PathBuf, cwd: PathBuf, grace: Duration) -> Self {
        Self {
            program,
            cwd,
            grace,
            cached: Mutex::new(None),
        }
    }

    /// The models, as pi lists them.
    ///
    /// # Errors
    ///
    /// Fails if pi can't be started or doesn't answer.
    pub async fn models(&self) -> Result<Arc<Vec<Model>>> {
        Ok(self.fetched().await?.models)
    }

    /// The model pi starts a chat on when none is picked, as `provider/model`,
    /// if it has one.
    ///
    /// # Errors
    ///
    /// Fails if pi can't be started or doesn't answer.
    pub async fn default_model(&self) -> Result<Option<String>> {
        Ok(self.fetched().await?.default)
    }

    async fn fetched(&self) -> Result<Fetched> {
        let mut cached = self.cached.lock().await;
        if let Some((at, fetched)) = &*cached
            && at.elapsed() < FRESH
        {
            return Ok(fetched.clone());
        }
        let fetched = self.fetch().await?;
        *cached = Some((Instant::now(), fetched.clone()));
        Ok(fetched)
    }

    /// The model `id`, if pi lists it.
    ///
    /// # Errors
    ///
    /// Fails if the models can't be fetched.
    pub async fn find(&self, id: &str) -> Result<Option<Model>> {
        let models = self.models().await?;
        Ok(models.iter().find(|model| model.id == id).cloned())
    }

    /// Ask a new pi for its models and the one it starts on.
    async fn fetch(&self) -> Result<Fetched> {
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
        Ok(Fetched {
            models: Arc::new(models.iter().filter_map(Model::from_pi).collect()),
            default,
        })
    }
}

/// What one pi said: its models, and the one it starts on.
#[derive(Debug, Clone)]
struct Fetched {
    models: Arc<Vec<Model>>,
    default: Option<String>,
}
