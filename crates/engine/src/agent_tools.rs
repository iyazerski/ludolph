//! Ludolph's own pi extensions, and the command-line tools its GUI extension drives.
//!
//! The extensions ship inside Ludolph.app and load only into the chats the engine runs, so a
//! terminal pi doesn't load them.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::settings::Settings;

/// Cua Driver's permission guide waits for the user to flip both switches.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The `--extension` paths and environment for a chat's pi.
pub fn pi_options(dir: &Path, settings: &Settings) -> (Vec<PathBuf>, Vec<(String, String)>) {
    let mut extensions = vec![dir.join("background-jobs.js")];
    let mut env = Vec::new();
    if settings.tool_routing {
        extensions.push(dir.join("routing.js"));
    }
    if settings.second_opinion {
        let reviewers = [
            (
                "LUDOLPH_REVIEW_CLAUDE_MODEL",
                "LUDOLPH_REVIEW_CLAUDE_THINKING",
                &settings.second_opinion_claude,
            ),
            (
                "LUDOLPH_REVIEW_GPT_MODEL",
                "LUDOLPH_REVIEW_GPT_THINKING",
                &settings.second_opinion_gpt,
            ),
        ];
        for (model, thinking, choice) in reviewers {
            if let Some(choice) = choice {
                env.push((model.to_owned(), choice.model.clone()));
                env.push((thinking.to_owned(), choice.thinking.clone()));
            }
        }
        if !env.is_empty() {
            extensions.push(dir.join("second-opinion.js"));
        }
    }
    let gui: Vec<&str> = [
        (settings.browser_use, "browser"),
        (settings.computer_use, "computer"),
    ]
    .into_iter()
    .filter_map(|(on, role)| on.then_some(role))
    .collect();
    if !gui.is_empty() {
        extensions.push(dir.join("gui.js"));
        env.push(("LUDOLPH_GUI_TOOLS".to_owned(), gui.join(",")));
        if let Some(model) = &settings.gui_model {
            env.push(("LUDOLPH_GUI_MODEL".to_owned(), model.clone()));
        }
        if let Some(thinking) = &settings.gui_thinking {
            env.push(("LUDOLPH_GUI_THINKING".to_owned(), thinking.clone()));
        }
    }
    (extensions, env)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuiTool {
    Browser,
    Computer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Missing,
    NeedsPermissions,
    /// browse is installed, but no browser has added its extension, so its tools can't run yet.
    Unpaired,
    Ready,
}

/// How a setup ended. A failure keeps the command's output for the user to copy, not to read inline.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Setup {
    Done(Status),
    Failed { error: String, log: Option<String> },
}

#[derive(Debug)]
struct CommandFailed {
    status: ExitStatus,
    log: String,
}

impl fmt::Display for CommandFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status.code() {
            Some(code) => write!(f, "it exited with code {code}"),
            None => write!(f, "it was stopped by a signal"),
        }
    }
}

impl std::error::Error for CommandFailed {}

#[derive(Deserialize)]
struct Permissions {
    accessibility: bool,
    screen_recording: bool,
}

impl GuiTool {
    fn command(self) -> &'static str {
        match self {
            Self::Browser => "browse",
            Self::Computer => "cua-driver",
        }
    }

    fn installer(self) -> &'static str {
        match self {
            // The extension runs browse in a subagent, so no agent's config gets it. Pairing with
            // the browser is a step of its own in `set_up`.
            Self::Browser => {
                "curl -fsSL https://opencode.ai/browse/install | bash -s -- --agent none --no-wait --no-open"
            }
            Self::Computer => "curl -fsSL https://cua.ai/driver/install.sh | bash",
        }
    }

    pub async fn status(self) -> Result<Status> {
        // pi starts the tool's MCP server by name, so only a command on PATH counts.
        let Some(program) = on_path(self.command()) else {
            return Ok(Status::Missing);
        };
        match self {
            Self::Browser => {
                let paired = run(Command::new(program).args(["pair", "--list"]))
                    .await
                    .context("browse couldn't list its paired browsers")?;
                Ok(if paired.starts_with("No paired browsers") {
                    Status::Unpaired
                } else {
                    Status::Ready
                })
            }
            Self::Computer => {
                let output = run(Command::new(program).args(["permissions", "status", "--json"]))
                    .await
                    .context("cua-driver couldn't check its permissions")?;
                let granted: Permissions =
                    serde_json::from_str(&output).context("reading cua-driver's permissions")?;
                Ok(if granted.accessibility && granted.screen_recording {
                    Status::Ready
                } else {
                    Status::NeedsPermissions
                })
            }
        }
    }

    /// Install the tool if it's missing, then ask for the macOS permissions it needs, or pair
    /// browse with a browser.
    pub async fn set_up(self) -> Setup {
        match self.try_set_up().await {
            Ok(status) => Setup::Done(status),
            Err(e) => match e.downcast_ref::<CommandFailed>() {
                // The log has the command's own words, so only the step that failed is shown.
                Some(failed) => Setup::Failed {
                    error: e.to_string(),
                    log: Some(failed.log.clone()),
                },
                None => Setup::Failed {
                    error: format!("{e:#}"),
                    log: None,
                },
            },
        }
    }

    async fn try_set_up(self) -> Result<Status> {
        let command = self.command();
        if self.status().await? == Status::Missing {
            run(Command::new("/bin/bash").args(["-c", self.installer()]))
                .await
                .with_context(|| format!("{command}'s installer failed"))?;
        }
        match self.status().await? {
            Status::Missing => bail!(
                "{command} installed, but it isn't on PATH. Add its folder to PATH in your shell profile, then restart Ludolph."
            ),
            Status::NeedsPermissions => {
                let program = on_path(command).context("cua-driver disappeared")?;
                // Opens a guide through both switches in System Settings and waits until it's done.
                run(Command::new(program).args(["permissions", "grant"]))
                    .await
                    .context("cua-driver couldn't ask for Accessibility and Screen Recording")?;
                self.status().await
            }
            Status::Unpaired => {
                let program = on_path(command).context("browse disappeared")?;
                // Opens the Chrome Web Store, or the extension's setup page with a one-time code
                // once it's added, and waits for the browser to connect. It fails when none does.
                let paired = run(Command::new(program).args([
                    "install",
                    "--agent",
                    "none",
                    "--no-modify-path",
                    "--wait-seconds",
                    "180",
                ]))
                .await;
                let status = self.status().await?;
                if status == Status::Unpaired {
                    paired.context(
                        "no browser connected in time. Add the browse extension in Chrome, then try again",
                    )?;
                }
                Ok(status)
            }
            Status::Ready => Ok(Status::Ready),
        }
    }
}

fn on_path(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|program| program.is_file())
}

/// Run a command and return its stdout, or fail with all of its output.
async fn run(command: &mut Command) -> Result<String> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let output = tokio::time::timeout(SETUP_TIMEOUT, child.wait_with_output())
        .await
        .with_context(|| format!("it took over {} minutes", SETUP_TIMEOUT.as_secs() / 60))??;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let both = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
        return Err(CommandFailed {
            status: output.status,
            log: plain(&both),
        }
        .into());
    }
    Ok(stdout)
}

/// Terminal output as plain text: no color codes, and of the lines a progress bar redraws with
/// `\r`, only the last.
fn plain(output: &str) -> String {
    let mut text = String::with_capacity(output.len());
    let mut chars = output.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            text.push(c);
        } else if chars.next() == Some('[') {
            // A CSI sequence ends at its first byte in @..=~.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    text.lines()
        .map(|line| {
            line.rsplit('\r')
                .find(|part| !part.is_empty())
                .unwrap_or("")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
