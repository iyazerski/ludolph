//! The user's skills and extensions, listed and switched by `resources.js` inside pi, so pi's own
//! resolver and settings writer decide what loads, as `pi config` does.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    ludolph_resources: Reply,
}

#[derive(Deserialize)]
struct Reply {
    resources: Option<Value>,
    error: Option<String>,
}

/// Applies `change` if given, then answers with every resource and whether it loads.
pub async fn call(
    program: &Path,
    extensions: &Path,
    grace: Duration,
    change: Option<&Change>,
) -> Result<Value> {
    let options = Options {
        program: program.to_owned(),
        // A folder with no project resources, since Settings aren't tied to a project.
        cwd: "/".into(),
        session: Session::None,
        extensions: vec![extensions.join("resources.js")],
        env: Vec::new(),
    };
    let (pi, mut events) = Pi::start(&options)?;
    let mut message = "/ludolph-resources".to_owned();
    if let Some(change) = change {
        message.push(' ');
        message.push_str(&serde_json::to_string(change)?);
    }
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
            let reply = answer.ludolph_resources;
            if let Some(error) = reply.error {
                bail!("{error}");
            }
            return reply
                .resources
                .context("resources.js answered without resources");
        }
        bail!("pi exited before listing its resources")
    };
    let answer = tokio::time::timeout(TIMEOUT, answer).await;
    if let Err(e) = pi.stop(grace).await {
        eprintln!("stopping the pi that listed resources: {e:#}");
    }
    answer.map_err(|_| anyhow!("pi didn't list its resources in {TIMEOUT:?}"))?
}
