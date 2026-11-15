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

const FRESH: Duration = Duration::from_mins(10);

/// pi's thinking levels, in order (`getSupportedThinkingLevels` in pi-ai).
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub levels: Vec<String>,
}

impl Model {
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

/// Match pi's level map: reasoning alone does not imply `xhigh` or `max` support.
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

#[derive(Debug)]
pub struct Catalog {
    program: PathBuf,
    /// Use a folder without project settings when discovering global models.
    cwd: PathBuf,
    grace: Duration,
    /// Concurrent catalog requests share one pi process.
    cached: Mutex<Option<(Instant, Sources, Available)>>,
    files: [PathBuf; 3],
    observed: StdMutex<Sources>,
    changes: watch::Sender<u64>,
}

impl Catalog {
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

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub(crate) fn sources(&self) -> Result<Sources> {
        let sources = Sources::read(&self.files)?;
        let mut observed = self.observed.lock().expect("lock poisoned");
        if *observed != sources {
            *observed = sources.clone();
            self.changes.send_modify(|revision| *revision += 1);
        }
        Ok(sources)
    }

    pub(crate) fn watches(&self, path: &Path) -> bool {
        self.files.iter().any(|file| file == path)
    }

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

    pub async fn find(&self, id: &str) -> Result<Option<Model>> {
        let available = self.available().await?;
        Ok(available
            .models
            .iter()
            .find(|model| model.id == id)
            .cloned())
    }

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

#[derive(Debug, Clone)]
pub struct Available {
    /// pi omits this for its Medium default.
    pub reasoning: String,
    pub models: Arc<Vec<Model>>,
    pub default: Option<String>,
}
