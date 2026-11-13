//! The models pi can use, for the composer's picker.
//!
//! pi lists them only over RPC (`get_available_models`), so the catalog asks
//! a short-lived pi without a session, and keeps the answer for a while: the
//! list changes only when someone edits pi's settings or logs in.

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
    cached: Mutex<Option<(Instant, Arc<Vec<Model>>)>>,
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
        let mut cached = self.cached.lock().await;
        if let Some((at, models)) = &*cached
            && at.elapsed() < FRESH
        {
            return Ok(models.clone());
        }
        let models = Arc::new(self.fetch().await?);
        *cached = Some((Instant::now(), models.clone()));
        Ok(models)
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

    /// Ask a new pi for its models.
    async fn fetch(&self) -> Result<Vec<Model>> {
        let command = json!({"type": "get_available_models"});
        let answer = Pi::ask(&self.program, &self.cwd, self.grace, command)
            .await
            .context("listing pi's models")?;
        let models = answer["models"].as_array().context("pi listed no models")?;
        Ok(models.iter().filter_map(Model::from_pi).collect())
    }
}
