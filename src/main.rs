//! The `ludolph` binary. `ludolph engine` runs the engine, `ludolph
//! install` runs it in the background from now on, and `ludolph link` links
//! this Mac to a relay for the phone.

mod agent;
mod shell;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ludolph_engine::engine::{Config, Engine};
use ludolph_engine::relay::{self, Link};
use ludolph_engine::server;
use ludolph_relay_proto::LinkCode;
use tokio::signal::unix::{SignalKind, signal};

/// How long a settled chat keeps its pi (`docs/design.md`).
const IDLE: Duration = Duration::from_secs(5 * 60);
/// How long pi gets to exit before it's killed.
const GRACE: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["engine"] => {
            // SAFETY: the engine's runtime, and so its threads, start after.
            unsafe { shell::adopt_login_env() };
            engine()
        }
        ["install"] => install(),
        ["uninstall"] => uninstall(),
        ["link", code] => link(code),
        ["unlink"] => unlink(),
        _ => {
            eprintln!(
                "usage: ludolph engine\n       ludolph install\n       ludolph uninstall\n       ludolph link <code>\n       ludolph unlink"
            );
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
    let home = home()?;
    let state_dir = state_dir(&home);
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
    };
    std::fs::create_dir_all(&config.state_dir)
        .with_context(|| format!("creating {}", config.state_dir.display()))?;
    let socket = config.state_dir.join("engine.sock");
    let listener = server::bind(&socket)?;
    let link = Link::load(&config.state_dir)?;
    let engine = Engine::start(config).await?;
    if let Some(link) = link {
        tokio::spawn(relay::run(engine.clone(), link));
    }
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

/// Run the engine in the background from now on, on this binary.
fn install() -> Result<()> {
    let log = agent::install(&home()?)?;
    println!(
        "The engine runs in the background now, and starts at login. It logs to {}.",
        log.display()
    );
    Ok(())
}

/// Stop the background engine and remove its launch agent.
fn uninstall() -> Result<()> {
    if agent::uninstall(&home()?)? {
        println!("The engine stopped, and won't start at login.");
    } else {
        println!("The engine isn't installed.");
    }
    Ok(())
}

/// Check that the relay takes the phone's link code, save the link, and
/// restart the installed engine so it connects.
#[tokio::main]
async fn link(code: &str) -> Result<()> {
    let LinkCode { relay, token } = LinkCode::decode(code)
        .context("that isn't a link code: copy it again from Add a Mac on the phone")?;
    let link = Link { relay, token };
    relay::verify(&link).await?;
    let home = home()?;
    let state_dir = state_dir(&home);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;
    link.save(&state_dir)?;
    // Until there are settings, the engine reads the link when it starts.
    if restart_installed(&home)? {
        println!("Linked to {}. The engine is connecting.", link.relay);
    } else {
        println!(
            "Linked to {}. The engine connects when it starts.",
            link.relay
        );
    }
    Ok(())
}

/// Forget the relay link.
fn unlink() -> Result<()> {
    let home = home()?;
    let path = Link::path(&state_dir(&home));
    match std::fs::remove_file(&path) {
        Ok(()) => {
            restart_installed(&home)?;
            println!("Unlinked.");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("This Mac isn't linked."),
        Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
    }
    Ok(())
}

fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME isn't set")?,
    ))
}

/// The engine's folder. `LUDOLPH_DIR` keeps a development engine apart from
/// the installed one.
/// Restart the installed engine if it uses this folder, which it doesn't
/// when `LUDOLPH_DIR` points at a development one. Returns whether it did.
fn restart_installed(home: &Path) -> Result<bool> {
    if std::env::var_os("LUDOLPH_DIR").is_some() {
        return Ok(false);
    }
    agent::restart(home)
}

fn state_dir(home: &Path) -> PathBuf {
    std::env::var_os("LUDOLPH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Library/Application Support/Ludolph"))
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
