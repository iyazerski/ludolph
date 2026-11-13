//! The engine's launch agent, which starts it at login and restarts it
//! after a crash.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

const LABEL: &str = "dev.iyazerski.ludolph.engine";

/// Install the agent to run this binary's engine, replacing an earlier one,
/// and start it. Returns the log's path.
///
/// # Errors
///
/// Fails if the plist can't be written, or launchd refuses it.
pub fn install(home: &std::path::Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the ludolph binary")?;
    let (plist, log) = (
        plist_path(home),
        home.join("Library/Logs/Ludolph/engine.log"),
    );
    for dir in [plist.parent(), log.parent()].into_iter().flatten() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let agent = LaunchAgent {
        label: LABEL,
        program_arguments: vec![exe.to_string_lossy().into_owned(), "engine".into()],
        run_at_load: true,
        keep_alive: KeepAlive {
            successful_exit: false,
        },
        throttle_interval: 30,
        standard_out_path: log.clone(),
        standard_error_path: log.clone(),
    };
    unload()?;
    plist::to_file_xml(&plist, &agent).with_context(|| format!("writing {}", plist.display()))?;
    launchctl(&["bootstrap", &domain(), &plist.to_string_lossy()])?;
    Ok(log)
}

/// Stop and remove the agent. Returns whether one was installed.
///
/// # Errors
///
/// Fails if launchd doesn't let the engine go, or the plist can't be
/// removed.
pub fn uninstall(home: &std::path::Path) -> Result<bool> {
    let plist = plist_path(home);
    if !plist.exists() {
        return Ok(false);
    }
    unload()?;
    std::fs::remove_file(&plist).with_context(|| format!("removing {}", plist.display()))?;
    Ok(true)
}

/// Restart the installed engine, so it reads its files again. Returns
/// whether one is installed.
///
/// # Errors
///
/// Fails if launchd can't restart it.
pub fn restart(home: &std::path::Path) -> Result<bool> {
    if !plist_path(home).exists() {
        return Ok(false);
    }
    launchctl(&["kickstart", "-k", &format!("{}/{LABEL}", domain())])?;
    Ok(true)
}

/// Stop the agent's job and wait until launchd lets it go: `bootout`
/// returns before that, and `bootstrap` fails while the label is loaded.
fn unload() -> Result<()> {
    let target = format!("{}/{LABEL}", domain());
    // Not loaded is fine.
    drop(launchctl(&["bootout", &target]));
    for _ in 0..100 {
        if launchctl(&["print", &target]).is_err() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("launchd didn't stop {target} in 10 s")
}

fn plist_path(home: &std::path::Path) -> PathBuf {
    home.join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

/// The job, as launchd reads it from the plist.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LaunchAgent {
    label: &'static str,
    program_arguments: Vec<String>,
    run_at_load: bool,
    keep_alive: KeepAlive,
    /// The fewest seconds between starts, so an engine that fails at once
    /// (say, another already serves the socket) doesn't spin.
    throttle_interval: u32,
    standard_out_path: PathBuf,
    standard_error_path: PathBuf,
}

/// Restart after a crash, but not after the clean exit SIGTERM brings.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct KeepAlive {
    successful_exit: bool,
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
