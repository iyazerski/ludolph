//! The `ludolph` binary. `ludolph` runs the windows, as Ludolph.app does,
//! and starts the engine in the background; `ludolph engine` is the engine,
//! and `ludolph link` links this Mac to a relay for the phone.

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
        [] => app(false),
        ["--hidden"] => app(true),
        ["uninstall"] => uninstall(),
        ["link", code] => link(code),
        ["unlink"] => unlink(),
        _ => {
            eprintln!(
                "usage: ludolph [--hidden]\n       ludolph engine\n       ludolph uninstall\n       ludolph link <code>\n       ludolph unlink"
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
    let exe = std::env::current_exe().context("finding the ludolph binary")?;
    // A development engine leaves the installed app's login item alone.
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

/// Run the windows until the app quits. It opens the main window, unless
/// `hidden` keeps it closed until the app is opened again, as at login.
fn app(hidden: bool) -> Result<()> {
    let home = home()?;
    let state_dir = state_dir(&home);
    // Outside the bundle, or with a development folder, you run the engine yourself.
    let exe = std::env::current_exe().context("finding the ludolph binary")?;
    if bundle_of(&exe).is_some() && std::env::var_os("LUDOLPH_DIR").is_none() {
        let state_dir = state_dir.clone();
        // launchd may take seconds to let an old engine go; the windows don't wait.
        std::thread::spawn(move || {
            if let Err(e) = agent::start(&home, &state_dir) {
                eprintln!("ludolph: {e:#}");
            }
        });
    }
    println!(
        "Ludolph is running; {} shows the quick window.",
        ludolph_ui::SHORTCUT
    );
    ludolph_ui::run(ludolph_ui::Opts { state_dir, hidden })
}

/// Stop the engine and remove the app's login item, before you delete
/// Ludolph.app.
fn uninstall() -> Result<()> {
    let stopped = agent::stop()?;
    let item = ludolph_engine::login::path(&home()?);
    let removed = match std::fs::remove_file(&item) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).with_context(|| format!("removing {}", item.display())),
    };
    println!(
        "{} {}",
        if stopped {
            "The engine stopped."
        } else {
            "The engine wasn't running."
        },
        if removed {
            "Ludolph won't open at login."
        } else {
            "Ludolph wasn't set to open at login."
        }
    );
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
    if restart_installed()? {
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
            restart_installed()?;
            println!("Unlinked.");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("This Mac isn't linked."),
        Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
    }
    Ok(())
}

/// The `.app` bundle `exe` runs from, if it runs from one, as in
/// `Ludolph.app/Contents/MacOS/ludolph`.
fn bundle_of(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .nth(3)
        .filter(|app| app.extension().is_some_and(|e| e == "app"))
}

fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME isn't set")?,
    ))
}

/// Restart the app's engine if it uses this folder, which it doesn't when
/// `LUDOLPH_DIR` points at a development one. Returns whether it did.
fn restart_installed() -> Result<bool> {
    if std::env::var_os("LUDOLPH_DIR").is_some() {
        return Ok(false);
    }
    agent::restart()
}

/// The engine's folder. `LUDOLPH_DIR` keeps a development engine apart from
/// the installed one.
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
