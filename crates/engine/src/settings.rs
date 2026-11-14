//! Ludolph's settings, which the engine keeps in `settings.json` and the
//! app changes through it (`docs/design.md`, "Settings").

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// What you can set. A missing field takes its default, so a file from an
/// older Ludolph still loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// The model for titles and dictation cleanup, as `provider/model`, run
    /// without thinking. pi's default model if `None`.
    pub sidecar_model: Option<String>,
    pub notify: Notify,
    /// Open Ludolph.app, hidden, at each login, and with it the engine. Off
    /// until you turn it on.
    pub open_at_login: bool,
    /// Show the work under way open while the agent runs, on the Mac and
    /// the phone alike. It folds once the agent writes again either way.
    pub expand_tools: bool,
    /// Hold messages until the run finishes, or steer the running agent.
    pub send_behavior: SendBehavior,
}

/// What Send does while the agent works.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendBehavior {
    /// Keep the message editable until its own turn starts.
    #[default]
    Queue,
    /// Hand it to pi after the current tool step.
    Steer,
}

/// When the phone gets notifications.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Notify {
    /// Only while nobody uses the Mac: at it, you see the chat yourself.
    #[default]
    Away,
    /// Whether or not someone uses the Mac.
    Always,
}

impl Settings {
    /// Where the settings are kept in the engine's folder.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("settings.json")
    }

    /// The saved settings, or the defaults if there are none yet.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read or isn't valid settings.
    pub fn load(state_dir: &Path) -> Result<Self> {
        Ok(ludolph_json_file::load(&Self::path(state_dir))?.unwrap_or_default())
    }

    /// Save the settings.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be written.
    pub fn save(&self, state_dir: &Path) -> Result<()> {
        ludolph_json_file::save(&Self::path(state_dir), self)
    }
}
