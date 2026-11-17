//! The app's link to a Linux engine, run locally or in WSL by the Windows app, which can't reach
//! a socket inside WSL. It joins stdin and stdout to the engine's socket, and starts the engine
//! first if none answers, as launchd does on macOS.

use std::fs::OpenOptions;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::runtime::Builder;
use tokio::time::Instant;

/// Reading the login shell and reaping old pi processes may each take up to 10 s.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

pub fn run(state_dir: &Path) -> Result<()> {
    let runtime = Builder::new_current_thread().enable_all().build()?;
    let result = runtime.block_on(relay(state_dir));
    // Tokio reads stdin on a blocking thread, which a closed engine leaves waiting.
    runtime.shutdown_background();
    result
}

async fn relay(state_dir: &Path) -> Result<()> {
    let socket = state_dir.join("engine.sock");
    let engine = match UnixStream::connect(&socket).await {
        Ok(stream) => stream,
        Err(_) => start(state_dir, &socket).await?,
    };
    let (mut from_engine, mut to_engine) = engine.into_split();
    let mut stdout = tokio::io::stdout();
    // An empty line tells the app the engine answered.
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    tokio::spawn(async move {
        if tokio::io::copy(&mut tokio::io::stdin(), &mut to_engine)
            .await
            .is_ok()
        {
            // The app is gone: the engine then closes its end too.
            drop(to_engine.shutdown().await);
        }
    });
    // Ends when the engine closes, so the app sees it and reconnects.
    tokio::io::copy(&mut from_engine, &mut stdout)
        .await
        .context("relaying from the engine")?;
    Ok(())
}

async fn start(state_dir: &Path, socket: &Path) -> Result<UnixStream> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;
    let log_path = state_dir.join("engine.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    // Not killed with this process: the engine outlives the app. Its own process group keeps
    // a terminal's signals for the app away from it.
    Command::new(std::env::current_exe().context("finding the ludolph binary")?)
        .arg("engine")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .context("starting the engine")?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(stream) = UnixStream::connect(socket).await {
            return Ok(stream);
        }
        if Instant::now() > deadline {
            bail!("the engine didn't start; see {}", log_path.display());
        }
        tokio::time::sleep(POLL).await;
    }
}
