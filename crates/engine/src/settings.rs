use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// `provider/model`; None uses pi's default for titles and dictation cleanup.
    pub sidecar_model: Option<String>,
    pub notify: Notify,
    pub open_at_login: bool,
    pub expand_tools: bool,
    pub send_behavior: SendBehavior,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendBehavior {
    /// Keep the message editable until its own turn starts.
    #[default]
    Queue,
    /// Hand it to pi after the current tool step.
    Steer,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Notify {
    #[default]
    Away,
    Always,
}

impl Settings {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("settings.json")
    }

    pub fn load(state_dir: &Path) -> Result<Self> {
        Ok(ludolph_json_file::load(&Self::path(state_dir))?.unwrap_or_default())
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        ludolph_json_file::save(&Self::path(state_dir), self)
    }
}
