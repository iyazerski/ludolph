//! Update operations block on system curl and filesystem work; run them off the UI thread.

#[cfg(target_os = "macos")]
mod bundle;
#[cfg(any(windows, test))]
mod signed;
#[cfg(windows)]
mod windows;

use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[cfg(target_os = "macos")]
pub use bundle::{install, relaunch};
#[cfg(windows)]
pub use windows::{install, relaunch};

const REPO: &str = "iyazerski/ludolph";

pub fn available() -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let body = curl(&[
        "-H",
        "Accept: application/vnd.github+json",
        // Overrides curl()'s download limit: quitting waits for a stuck check.
        "--max-time",
        "30",
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
    ])?;
    let release: Release = serde_json::from_slice(&body).context("reading the latest release")?;
    let latest = release.tag_name.trim_start_matches('v');
    Ok(newer(latest, crate::VERSION)?.then(|| latest.to_owned()))
}

/// Linux users install updates from the release page.
#[cfg(target_os = "linux")]
pub fn release_page(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{version}")
}

fn newer(candidate: &str, current: &str) -> Result<bool> {
    let parse =
        |v: &str| semver::Version::parse(v).with_context(|| format!("unexpected version {v}"));
    Ok(parse(candidate)? > parse(current)?)
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/curl");
    // Windows has shipped curl since Windows 10 1803.
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new("curl");
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        &mut command,
        windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
    );
    let output = command
        .args(["-fsSL", "--max-time", "300", "-A", "Ludolph"])
        .args(args)
        .output()
        .context("running curl")?;
    if !output.status.success() {
        bail!(
            "downloading {} failed: {}",
            args.last().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}
