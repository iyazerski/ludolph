use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

const LABEL: &str = "dev.iyazerski.ludolph";

pub fn path(home: &Path) -> PathBuf {
    home.join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

/// Leave the installed login item alone when running outside Ludolph.app.
pub fn apply(home: &Path, app: Option<&Path>, on: bool) -> Result<()> {
    let path = path(home);
    let Some(app) = app else {
        if on {
            bail!("only Ludolph.app can open at login, and this engine doesn't run from it");
        }
        return Ok(());
    };
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", path.display()))
            }
            _ => Ok(()),
        };
    }
    let item = LoginItem {
        label: LABEL,
        // Through LaunchServices, so opening Ludolph.app later reaches this copy.
        program_arguments: vec![
            "/usr/bin/open".into(),
            "-g".into(),
            "-a".into(),
            app.to_string_lossy().into_owned(),
            "--args".into(),
            "--hidden".into(),
        ],
        run_at_load: true,
        process_type: "Interactive",
    };
    let mut xml = Vec::new();
    plist::to_writer_xml(&mut xml, &item).context("writing the login item")?;
    // An unchanged file stays as it is, so macOS doesn't announce a new background item.
    if std::fs::read(&path).is_ok_and(|old| old == xml) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, xml).with_context(|| format!("writing {}", path.display()))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LoginItem {
    label: &'static str,
    program_arguments: Vec<String>,
    run_at_load: bool,
    /// An app the user sees, not a background job launchd may hold back.
    process_type: &'static str,
}
