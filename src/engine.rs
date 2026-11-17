use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ludolph_engine::engine::{Config, Engine, SecretStore};
use ludolph_engine::server;
use tokio::signal::unix::{SignalKind, signal};

const IDLE: Duration = Duration::from_secs(5 * 60);
const GRACE: Duration = Duration::from_secs(10);

#[tokio::main]
pub async fn run(state_dir: PathBuf) -> Result<()> {
    let home = crate::home()?;
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".pi/agent"));
    let exe = std::env::current_exe().context("finding the ludolph binary")?;
    let app = bundle_of(&exe)
        .filter(|_| std::env::var_os("LUDOLPH_DIR").is_none())
        .map(Path::to_owned);
    let config = Config {
        pi: find_pi(&home)?,
        sessions_dir: agent_dir.join("sessions"),
        state_dir,
        idle: IDLE,
        grace: GRACE,
        home,
        app,
        extensions: extensions_dir(&exe)?,
        #[cfg(target_os = "macos")]
        secrets: SecretStore::Keychain,
        #[cfg(not(target_os = "macos"))]
        secrets: SecretStore::File,
    };
    std::fs::create_dir_all(&config.state_dir)
        .with_context(|| format!("creating {}", config.state_dir.display()))?;
    // launchd runs one engine on macOS. Elsewhere each `ludolph connect` may start one.
    #[cfg(not(target_os = "macos"))]
    let _lock = {
        let path = config.state_dir.join("engine.lock");
        let lock =
            std::fs::File::create(&path).with_context(|| format!("opening {}", path.display()))?;
        if lock.try_lock().is_err() {
            bail!("an engine is already running");
        }
        lock
    };
    let socket = config.state_dir.join("engine.sock");
    let listener = server::bind(&socket)?;
    let engine = Engine::start(config).await?;
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        result = server::serve(engine.clone(), listener) => result?,
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    // New clients get refused at once, rather than wait on a stopping engine.
    drop(std::fs::remove_file(&socket));
    engine.shutdown().await
}

pub fn bundle_of(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .nth(3)
        .filter(|app| app.extension().is_some_and(|e| e == "app"))
}

/// pi's extensions that Ludolph ships: in the app bundle, under the install prefix on Linux
/// (`~/.local/bin/ludolph` has them in `~/.local/share/ludolph/pi`), or, for development
/// builds, in the target/pi folder the Makefile builds.
fn extensions_dir(exe: &Path) -> Result<PathBuf> {
    if let Some(bundle) = bundle_of(exe) {
        return Ok(bundle.join("Contents/Resources/pi"));
    }
    let dir = exe.parent().context("finding the binary's folder")?;
    if dir.ends_with("bin") {
        return Ok(dir.with_file_name("share/ludolph/pi"));
    }
    Ok(exe
        .ancestors()
        .nth(2)
        .context("finding the target folder")?
        .join("pi"))
}

/// launchd has a short PATH; also check pi's installer locations.
fn find_pi(home: &Path) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let extra = [".local/bin", ".bun/bin", ".npm-global/bin"].map(|dir| home.join(dir));
    let fixed = ["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from);
    let candidates = std::env::split_paths(&path).chain(extra).chain(fixed);
    for dir in candidates {
        let pi = dir.join("pi");
        if pi.is_file() {
            return Ok(pi);
        }
    }
    bail!("can't find pi: install it from https://pi.dev")
}
