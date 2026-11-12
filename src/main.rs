//! The `ludolph` binary. `ludolph engine` runs the engine.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ludolph_engine::engine::{Config, Engine};
use ludolph_engine::server;
use tokio::signal::unix::{SignalKind, signal};

/// How long a settled chat keeps its pi (`docs/design.md`).
const IDLE: Duration = Duration::from_secs(5 * 60);
/// How long pi gets to exit before it's killed.
const GRACE: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["engine"] => engine(),
        _ => {
            eprintln!("usage: ludolph engine");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ludolph: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Run the engine until SIGTERM or SIGINT, then stop its pi processes.
#[tokio::main]
async fn engine() -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set")?);
    // `LUDOLPH_DIR` keeps a development engine apart from the installed one.
    let state_dir = std::env::var_os("LUDOLPH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Library/Application Support/Ludolph"));
    // The same rule as pi's own.
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".pi/agent"));
    let config = Config {
        pi: find_pi(&home)?,
        sessions_dir: agent_dir.join("sessions"),
        state_dir,
        idle: IDLE,
        grace: GRACE,
        home,
        // Until there are settings (`docs/design.md`, "Titles").
        title_model: std::env::var("LUDOLPH_TITLE_MODEL").ok(),
        title_thinking: std::env::var("LUDOLPH_TITLE_THINKING").unwrap_or_else(|_| "low".into()),
    };
    std::fs::create_dir_all(&config.state_dir)
        .with_context(|| format!("creating {}", config.state_dir.display()))?;
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

/// The `pi` executable: on `PATH`, or where pi's installers put it, since a
/// launch agent's `PATH` is short.
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
