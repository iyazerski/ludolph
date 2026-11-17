// A GUI app on Windows, which opens no console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod agent;
#[cfg(target_os = "linux")]
mod connect;
#[cfg(unix)]
mod engine;
#[cfg(unix)]
mod shell;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};

#[cfg(target_os = "macos")]
const USAGE: &str = "usage: ludolph [--hidden]\n       ludolph engine\n       ludolph uninstall";
#[cfg(target_os = "linux")]
const USAGE: &str = "usage: ludolph [--hidden]\n       ludolph engine\n       ludolph connect";
#[cfg(windows)]
const USAGE: &str = "usage: ludolph [--hidden]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => app(false),
        ["--hidden"] => app(true),
        #[cfg(unix)]
        // The state folder comes first, so the login shell can't move it away from
        // where `ludolph connect` looks.
        ["engine"] => state_dir().and_then(|dir| {
            // SAFETY: the engine's runtime, and so its threads, start after.
            unsafe { shell::adopt_login_env() };
            engine::run(dir)
        }),
        #[cfg(target_os = "linux")]
        ["connect"] => state_dir().and_then(|dir| connect::run(&dir)),
        #[cfg(target_os = "macos")]
        ["uninstall"] => uninstall(),
        _ => {
            eprintln!("{USAGE}");
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

fn app(hidden: bool) -> Result<()> {
    let state_dir = state_dir()?;
    // Outside the bundle, or with a development folder, you run the engine yourself.
    // Elsewhere the app reaches the engine through `ludolph connect`, which starts it.
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().context("finding the ludolph binary")?;
        if engine::bundle_of(&exe).is_some() && std::env::var_os("LUDOLPH_DIR").is_none() {
            let (home, state_dir) = (home()?, state_dir.clone());
            // launchd may take seconds to let an old engine go; the windows don't wait.
            std::thread::spawn(move || {
                if let Err(e) = agent::start(&home, &state_dir) {
                    eprintln!("ludolph: {e:#}");
                }
            });
        }
    }
    ludolph_ui::run(ludolph_ui::Opts { state_dir, hidden })
}

#[cfg(target_os = "macos")]
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

#[cfg(unix)]
fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME isn't set")?,
    ))
}

fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("LUDOLPH_DIR") {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(target_os = "macos")]
    let dir = home()?.join("Library/Application Support/Ludolph");
    #[cfg(target_os = "linux")]
    let dir = match std::env::var_os("XDG_STATE_HOME").map(PathBuf::from) {
        Some(state) if state.is_absolute() => state,
        _ => home()?.join(".local/state"),
    }
    .join("ludolph");
    // The app's own state: the engine and its state are in WSL.
    #[cfg(windows)]
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA isn't set")?)
        .join("Ludolph");
    Ok(dir)
}
