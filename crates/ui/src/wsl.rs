use std::os::windows::process::CommandExt;
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use windows_sys::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW};

#[derive(Debug)]
pub enum Status {
    Checking,
    NeedsWsl(String),
    NeedsEngine(String),
    NeedsPi(String),
    Installing,
    Ready,
    Failed(String),
}

fn command() -> Command {
    let mut command = Command::new("wsl.exe");
    command.creation_flags(CREATE_NO_WINDOW).kill_on_drop(true);
    command
}

fn detail(output: &Output) -> String {
    let bytes = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    if bytes.contains(&0) {
        String::from_utf16_lossy(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect::<Vec<_>>(),
        )
        .trim_start_matches('\u{feff}')
        .trim()
        .to_owned()
    } else {
        String::from_utf8_lossy(bytes).trim().to_owned()
    }
}

pub async fn inspect() -> Result<Status> {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        command().args(["--cd", "~", "--exec", "sh", "-c", r#"
set -eu
printf '%s\n' "$WSL_DISTRO_NAME"
if [ "$("$HOME/.local/bin/ludolph" --version 2>/dev/null || true)" != "$1" ]; then
    echo engine
elif "$SHELL" -ilc 'command -v pi >/dev/null || test -x "$HOME/.local/bin/pi" || test -x "$HOME/.bun/bin/pi" || test -x "$HOME/.npm-global/bin/pi"' </dev/null >/dev/null 2>&1; then
    echo ready
else
    echo pi
fi
"#, "sh", crate::VERSION]).output(),
    ).await.context("WSL didn't answer within 30 seconds. Finish setting up its Linux user, then try again")?;
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Status::NeedsWsl(String::new()));
        }
        Err(error) => return Err(error).context("checking WSL"),
    };
    if !output.status.success() {
        return Ok(Status::NeedsWsl(detail(&output)));
    }
    let text = String::from_utf8(output.stdout).context("reading WSL's response")?;
    let mut lines = text.lines();
    let distribution = lines
        .next()
        .filter(|s| !s.is_empty())
        .context("WSL didn't name its distribution")?
        .to_owned();
    match lines.next() {
        Some("engine") => Ok(Status::NeedsEngine(distribution)),
        Some("pi") => Ok(Status::NeedsPi(distribution)),
        Some("ready") => Ok(Status::Ready),
        _ => bail!("unexpected WSL setup response"),
    }
}

pub async fn install(distribution: &str) -> Result<Status> {
    let exe = std::env::current_exe().context("finding Ludolph")?;
    let assets = exe
        .parent()
        .context("finding Ludolph's folder")?
        .join("wsl");
    let assets = assets
        .to_str()
        .context("the installation path isn't Unicode")?;
    let mut child = command()
        .args([
            "--distribution",
            distribution,
            "--cd",
            "~",
            "--exec",
            "sh",
            "-c",
            r#"assets=$(wslpath -a -u "$1") && exec sh -s -- "$assets" "$2""#,
            "sh",
            assets,
            crate::VERSION,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting engine setup in WSL")?;
    let mut stdin = child.stdin.take().context("opening WSL setup input")?;
    stdin
        .write_all(include_bytes!("../../../scripts/wsl-install.sh"))
        .await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
        .await
        .context("engine setup took too long. Check WSL, then try again")??;
    if !output.status.success() {
        bail!("couldn't install the engine: {}", detail(&output));
    }
    inspect().await
}

pub fn install_wsl() -> Result<()> {
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "Start-Process wsl.exe -Verb RunAs -ArgumentList '--install'",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("opening Windows' WSL installer")?;
    Ok(())
}

pub fn open_terminal() -> Result<()> {
    // A visible Linux shell lets the user install pi and sign in themselves.
    std::process::Command::new("wsl.exe")
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .context("opening the WSL terminal")?;
    Ok(())
}
