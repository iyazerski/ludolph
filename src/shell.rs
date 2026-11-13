//! The login shell's `PATH`. launchd starts the engine with a short one,
//! but pi's tools (`git`, `cargo`, `uv`, …) need what a terminal has.

use std::ffi::CStr;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// How long the shell may take: an interactive one loads your rc files.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Marks the value in the output, since rc files may print too.
const MARK: &str = "_LUDOLPH_PATH_";

/// Use the login shell's `PATH` from now on, or keep the current one if it
/// can't be read.
///
/// # Safety
///
/// No other thread may be running, since it changes the environment.
pub unsafe fn adopt_login_path() {
    match login_path() {
        // SAFETY: the caller promises there's only this thread.
        Ok(path) => unsafe { std::env::set_var("PATH", path) },
        Err(e) => eprintln!("keeping launchd's PATH: {e:#}"),
    }
}

fn login_path() -> Result<String> {
    let shell = user_shell().unwrap_or_else(|| "/bin/zsh".into());
    let fish = shell.file_name().is_some_and(|name| name == "fish");
    // fish keeps `PATH` as a list, which `"$PATH"` would join with spaces.
    let value = if fish {
        "(string join : $PATH)"
    } else {
        "\"$PATH\""
    };
    // Interactive and login, so both `.zprofile` and `.zshrc` edits count.
    let mut child = Command::new(&shell)
        .args([
            "-i",
            "-l",
            "-c",
            &format!("printf '\\n{MARK}%s{MARK}\\n' {value}"),
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
    let path = out.split(MARK).nth(1).filter(|path| !path.is_empty());
    path.map(str::to_owned)
        .with_context(|| format!("{} printed no PATH", shell.display()))
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
