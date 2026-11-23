//! pi's own setup for Settings: providers and their sign-in, and which skills and extensions load.
//! `pi-setup.js` answers from inside pi, so pi's resolver and settings writer decide what loads, as
//! `pi config` does.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use tokio::sync::mpsc;

use crate::pi::{Event, Options, Pi, Session};

const TIMEOUT: Duration = Duration::from_secs(30);

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
    #[serde(default)]
    ok: Value,
    error: Option<String>,
}

/// A pi that runs `pi-setup.js`, from a folder with no project resources, since Settings aren't
/// tied to a project.
pub(crate) fn start(program: &Path, extensions: &Path) -> Result<(Pi, mpsc::Receiver<Event>)> {
    Pi::start(&Options {
        program: program.to_owned(),
        cwd: "/".into(),
        session: Session::None,
        extensions: vec![extensions.join("pi-setup.js")],
        env: Vec::new(),
    })
}

pub(crate) fn command(request: &Value) -> Value {
    json!({"type": "prompt", "message": format!("/ludolph-setup {request}")})
}

/// pi-setup.js's answer, if `event` is it. Other extensions may notify too.
pub(crate) fn answer(event: &Event) -> Option<Result<Value>> {
    let Event::ExtensionUiRequest {
        method,
        message: Some(message),
        ..
    } = event
    else {
        return None;
    };
    if method != "notify" {
        return None;
    }
    let answer = serde_json::from_str::<Answer>(message).ok()?;
    let reply = answer.ludolph_setup;
    Some(match reply.error {
        Some(error) => Err(anyhow!("{error}")),
        None => Ok(reply.ok),
    })
}

pub async fn ask(
    program: &Path,
    extensions: &Path,
    grace: Duration,
    request: Value,
) -> Result<Value> {
    let (pi, mut events) = start(program, extensions)?;
    let answer = async {
        pi.call(command(&request)).await?;
        while let Some(event) = events.recv().await {
            if let Some(answer) = answer(&event) {
                return answer;
            }
        }
        bail!("pi exited before answering about its setup")
    };
    let answer = tokio::time::timeout(TIMEOUT, answer).await;
    if let Err(e) = pi.stop(grace).await {
        eprintln!("stopping the pi that answered about its setup: {e:#}");
    }
    answer.map_err(|_| anyhow!("pi didn't answer about its setup in {TIMEOUT:?}"))?
}
