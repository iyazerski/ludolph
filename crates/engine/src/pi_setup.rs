//! pi's own setup for Settings: providers and their sign-in, and which skills and extensions load.
//! `pi-setup.js` answers from inside pi, so pi's resolver and settings writer decide what loads, as
//! `pi config` does.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::pi::{Event, Options, Pi, Session};

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    Accounts,
    Resources,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub kind: Kind,
    pub path: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Extensions,
    Skills,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    ludolph_setup: Reply,
}

#[derive(Deserialize)]
struct Reply {
    ok: Option<Value>,
    error: Option<String>,
}

/// Applies `change`, if given, before answering about `topic`.
pub async fn ask(
    program: &Path,
    extensions: &Path,
    grace: Duration,
    topic: Topic,
    change: Option<&Change>,
) -> Result<Value> {
    let options = Options {
        program: program.to_owned(),
        // A folder with no project resources, since Settings aren't tied to a project.
        cwd: "/".into(),
        session: Session::None,
        extensions: vec![extensions.join("pi-setup.js")],
        env: Vec::new(),
    };
    let (pi, mut events) = Pi::start(&options)?;
    let args = json!({"topic": topic, "change": change});
    let message = format!("/ludolph-setup {args}");
    let answer = async {
        pi.call(json!({"type": "prompt", "message": message}))
            .await?;
        while let Some(event) = events.recv().await {
            let Event::ExtensionUiRequest {
                method,
                message: Some(message),
                ..
            } = event
            else {
                continue;
            };
            // Other extensions may notify too.
            let Ok(answer) = serde_json::from_str::<Answer>(&message) else {
                continue;
            };
            if method != "notify" {
                continue;
            }
            let reply = answer.ludolph_setup;
            if let Some(error) = reply.error {
                bail!("{error}");
            }
            return reply.ok.context("pi-setup.js answered with nothing");
        }
        bail!("pi exited before answering about its setup")
    };
    let answer = tokio::time::timeout(TIMEOUT, answer).await;
    if let Err(e) = pi.stop(grace).await {
        eprintln!("stopping the pi that answered about its setup: {e:#}");
    }
    answer.map_err(|_| anyhow!("pi didn't answer about its setup in {TIMEOUT:?}"))?
}
