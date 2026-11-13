//! Ludolph's launch agents: the engine's, which starts it at login and
//! restarts it after a crash, and the app's login item.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const LABEL: &str = "dev.iyazerski.ludolph.engine";
/// The app's login item, labelled like the app.
const LOGIN_LABEL: &str = "dev.iyazerski.ludolph";

/// Install the agent to run this binary's engine, replacing an earlier one,
/// and start it. Returns the log's path.
///
/// # Errors
///
/// Fails if the plist can't be written, or launchd refuses it.
pub fn install(home: &Path) -> Result<PathBuf> {
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

/// Point the engine's agent and the app's login item at `app`, the bundle
/// this binary runs from. The app does this each time it starts, so moving
/// or updating it moves both.
///
/// # Errors
///
/// Fails if a plist can't be written, or launchd refuses the engine.
pub fn adopt(home: &Path, app: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("finding the ludolph binary")?;
    let installed: Option<Installed> = plist::from_file(plist_path(home)).ok();
    let ours = installed.is_some_and(|i| i.program_arguments.first().map(Path::new) == Some(&exe));
    // Starts the engine if it isn't running, as after an update stopped it.
    if !ours || launchctl(&["kickstart", &format!("{}/{LABEL}", domain())]).is_err() {
        install(home)?;
    }
    login_item(home, app)
}

/// Start `app` hidden at every login. It isn't loaded now: that would start
/// it, and so reopen the copy already running.
fn login_item(home: &Path, app: &Path) -> Result<()> {
    let item = LoginItem {
        label: LOGIN_LABEL,
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
    let path = login_path(home);
    let mut xml = Vec::new();
    plist::to_writer_xml(&mut xml, &item).context("writing the login item")?;
    // Unchanged files stay as they are, so macOS doesn't announce a new background item.
    if std::fs::read(&path).is_ok_and(|old| old == xml) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, xml).with_context(|| format!("writing {}", path.display()))
}

/// Stop and remove the engine's agent and the app's login item. Returns
/// whether either was installed.
///
/// # Errors
///
/// Fails if launchd doesn't let the engine go, or a plist can't be removed.
pub fn uninstall(home: &Path) -> Result<bool> {
    let plist = plist_path(home);
    let engine = plist.exists();
    if engine {
        unload()?;
        std::fs::remove_file(&plist).with_context(|| format!("removing {}", plist.display()))?;
    }
    let login = login_path(home);
    let app = login.exists();
    if app {
        // Loaded since the last login, though its `open` is long done.
        drop(launchctl(&[
            "bootout",
            &format!("{}/{LOGIN_LABEL}", domain()),
        ]));
        std::fs::remove_file(&login).with_context(|| format!("removing {}", login.display()))?;
    }
    Ok(engine || app)
}

/// Restart the installed engine, so it reads its files again. Returns
/// whether one is installed.
///
/// # Errors
///
/// Fails if launchd can't restart it.
pub fn restart(home: &Path) -> Result<bool> {
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

fn plist_path(home: &Path) -> PathBuf {
    home.join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

fn login_path(home: &Path) -> PathBuf {
    home.join(format!("Library/LaunchAgents/{LOGIN_LABEL}.plist"))
}

/// What `adopt` reads back from the engine's installed plist.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Installed {
    program_arguments: Vec<String>,
}

/// The app's login item, as launchd reads it.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LoginItem {
    label: &'static str,
    program_arguments: Vec<String>,
    run_at_load: bool,
    /// An app the user sees, not a background job launchd may hold back.
    process_type: &'static str,
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
