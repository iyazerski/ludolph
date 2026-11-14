//! The engine's launchd job. Ludolph.app loads it each time it opens, for
//! this login session only: its plist is in Ludolph's folder, not in
//! `~/Library/LaunchAgents`, so nothing starts at the next login unless the
//! `openAtLogin` setting opens the app. launchd restarts the engine after a
//! crash, and keeps it running when the app quits, for the phone.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

const LABEL: &str = "dev.iyazerski.ludolph.engine";

/// Run this binary's engine for the rest of the session: load its job, or
/// reload it if the binary moved, or start it if it stopped, as after an
/// update.
///
/// # Errors
///
/// Fails if the plist can't be written, or launchd refuses it.
pub fn start(home: &Path, state_dir: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("finding the ludolph binary")?;
    let log = home.join("Library/Logs/Ludolph/engine.log");
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let job = Job {
        label: LABEL,
        program_arguments: vec![exe.to_string_lossy().into_owned(), "engine".into()],
        keep_alive: KeepAlive {
            successful_exit: false,
        },
        throttle_interval: 30,
        standard_out_path: log.clone(),
        standard_error_path: log,
        environment_variables: Env {
            ludolph_build: ludolph_engine::VERSION,
        },
    };
    let mut xml = Vec::new();
    plist::to_writer_xml(&mut xml, &job).context("writing the engine's job")?;
    let path = state_dir.join("engine.plist");
    let changed = std::fs::read(&path).map_or(true, |old| old != xml);
    if changed {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("creating {}", state_dir.display()))?;
        std::fs::write(&path, xml).with_context(|| format!("writing {}", path.display()))?;
    }
    if loaded() {
        if !changed {
            // Starts it if it isn't running; a running engine carries on.
            return launchctl(&["kickstart", &target()]);
        }
        unload()?;
    }
    launchctl(&["bootstrap", &domain(), &path.to_string_lossy()])
}

/// Stop the engine for the rest of the session. Returns whether it was
/// loaded.
///
/// # Errors
///
/// Fails if launchd doesn't let it go.
pub fn stop() -> Result<bool> {
    if !loaded() {
        return Ok(false);
    }
    unload()?;
    Ok(true)
}

fn loaded() -> bool {
    launchctl(&["print", &target()]).is_ok()
}

/// Stop the job and wait until launchd lets it go: `bootout` returns before
/// that, and `bootstrap` fails while the label is loaded.
fn unload() -> Result<()> {
    // Not loaded is fine.
    drop(launchctl(&["bootout", &target()]));
    for _ in 0..100 {
        if !loaded() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("launchd didn't stop {} in 10 s", target())
}

/// The job, as launchd reads it from the plist.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Job {
    label: &'static str,
    program_arguments: Vec<String>,
    /// Also starts the job when it's loaded.
    keep_alive: KeepAlive,
    /// The fewest seconds between starts, so an engine that fails at once
    /// (say, another already serves the socket) doesn't spin.
    throttle_interval: u32,
    standard_out_path: PathBuf,
    standard_error_path: PathBuf,
    environment_variables: Env,
}

/// The engine's environment, which names the version: an update replaces the
/// binary at the same path, and the changed plist then restarts the engine.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
struct Env {
    ludolph_build: &'static str,
}

/// Restart after a crash, but not after the clean exit SIGTERM brings.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct KeepAlive {
    successful_exit: bool,
}

fn target() -> String {
    format!("{}/{LABEL}", domain())
}

/// The launchd domain of your GUI session.
fn domain() -> String {
    // SAFETY: `getuid` has no preconditions and can't fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchctl(args: &[&str]) -> Result<()> {
    let out = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .context("running launchctl")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("launchctl {}: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}
