//! The login shell's environment. launchd starts the engine with a bare
//! one, but pi needs what a terminal has: a `PATH` for its tools (`git`,
//! `cargo`, `uv`, …), and the API keys and proxy settings rc files export.

use std::ffi::CStr;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// How long the shell may take: an interactive one loads your rc files.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Marks the values in the output, since rc files may print too.
const MARK: &str = "_LUDOLPH_ENV_";
/// The shell's own bookkeeping, which means nothing to the engine.
const SKIP: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

/// Add the login shell's environment to the engine's from now on, or keep
/// launchd's alone if it can't be read.
///
/// # Safety
///
/// No other thread may be running, since it changes the environment.
pub unsafe fn adopt_login_env() {
    match login_env() {
        Ok(vars) => {
            for (name, value) in vars {
                // SAFETY: the caller promises there's only this thread.
                unsafe { std::env::set_var(name, value) };
            }
        }
        Err(e) => eprintln!("keeping launchd's environment: {e:#}"),
    }
}

fn login_env() -> Result<Vec<(String, String)>> {
    let shell = user_shell().unwrap_or_else(|| "/bin/zsh".into());
    // Interactive and login, so both `.zprofile` and `.zshrc` edits count.
    // `env` prints the same in every shell, fish's list `PATH` included.
    let mut child = Command::new(&shell)
        .args([
            "-i",
            "-l",
            "-c",
            &format!("printf '{MARK}'; /usr/bin/env -0; printf '{MARK}'"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {}", shell.display()))?;
    let mut stdout = child.stdout.take().context("the shell has no stdout")?;
    // Read alongside waiting, so chatty rc files can't fill the pipe.
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).map(|_| out)
    });
    let deadline = Instant::now() + TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            drop(child.kill());
            drop(child.wait());
            bail!(
                "{} didn't answer in {}s",
                shell.display(),
                TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = reader.join().expect("the reader doesn't panic")?;
    let block = out.split(MARK).nth(1).unwrap_or_default();
    let vars: Vec<(String, String)> = block
        .split('\0')
        .filter_map(|var| var.split_once('='))
        .filter(|(name, _)| !name.is_empty() && !SKIP.contains(name))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    if !vars.iter().any(|(name, _)| name == "PATH") {
        bail!("{} printed no environment", shell.display());
    }
    Ok(vars)
}

/// Your login shell, from the user database rather than `SHELL`, which
/// launchd may not set.
fn user_shell() -> Option<PathBuf> {
    // SAFETY: `getpwuid` returns null or a record that stays valid until the
    // next call, and no other thread calls it.
    let record = unsafe { libc::getpwuid(libc::getuid()).as_ref() }?;
    if record.pw_shell.is_null() {
        return None;
    }
    // SAFETY: `pw_shell` is a NUL-terminated string in that record.
    let shell = unsafe { CStr::from_ptr(record.pw_shell) };
    Some(PathBuf::from(shell.to_str().ok()?))
}
