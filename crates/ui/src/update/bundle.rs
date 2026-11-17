//! Replacing Ludolph.app with a new release, in place.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

use super::{REPO, curl};

const ASSET: &str = "Ludolph.zip";
/// Keep this release-certificate hash in sync with install.sh and CI signing.
const SIGNER: &str = r#"=identifier "dev.iyazerski.ludolph" and certificate leaf = H"7c6d7b2fa7382af0dd7f9f3e31f6e016fbe2fd46""#;

pub fn install(version: &str) -> Result<PathBuf> {
    let app = running_app()?;
    let parent = app.parent().context("the app has no folder")?;
    // Beside the app, so the swap is a rename on one volume.
    let work = parent.join(format!(".ludolph-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir(&work)
        .with_context(|| format!("Ludolph can't write to {}", parent.display()))?;
    let result = unpack_and_swap(&app, &work, version);
    let _ = std::fs::remove_dir_all(&work);
    result.map(|()| app)
}

pub fn relaunch(app: &Path) -> Result<()> {
    Command::new("/bin/sh")
        .args([
            "-c",
            r#"while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; exec /usr/bin/open "$0""#,
        ])
        .arg(app)
        .arg(std::process::id().to_string())
        .spawn()
        .context("starting the new version")?;
    Ok(())
}

fn running_app() -> Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    exe.ancestors()
        .nth(3)
        .filter(|app| app.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
        .context("Ludolph isn't running from Ludolph.app, so it can't update itself")
}

fn unpack_and_swap(app: &Path, work: &Path, version: &str) -> Result<()> {
    let url = format!("https://github.com/{REPO}/releases/download/v{version}/{ASSET}");
    let zip = curl(&[&url])?;
    let sum = curl(&[&format!("{url}.sha256")])?;
    let expected = String::from_utf8_lossy(&sum);
    let expected = expected.split_whitespace().next().unwrap_or_default();
    let actual: String = ring::digest::digest(&ring::digest::SHA256, &zip)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ensure!(
        actual == expected,
        "the download is damaged: its checksum doesn't match"
    );
    let archive = work.join(ASSET);
    std::fs::write(&archive, zip)?;
    run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(&archive)
        .arg(work))?;
    let new = work.join("Ludolph.app");
    ensure!(new.is_dir(), "the download has no Ludolph.app");
    // The checksum comes from the same release, so it only catches damage. The
    // signature shows the app is Ludolph's own.
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R", SIGNER])
        .arg(&new))
    .context("the download isn't signed by Ludolph's release certificate")?;
    // One atomic exchange, so even a crash leaves one version in place.
    exchange(&new, app).context("putting the new version in place")
}

/// Swap the items at `a` and `b` in one step. Both must be on one volume.
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated paths that outlive the call.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn run(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    if !output.status.success() {
        bail!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SIGNER;

    #[test]
    fn trusts_the_certificate_install_sh_trusts() {
        let script = include_str!("../../../../install.sh");
        let leaf = script
            .lines()
            .find_map(|l| l.strip_prefix("leaf=\""))
            .and_then(|l| l.strip_suffix('"'))
            .expect("install.sh names the release certificate");
        assert!(SIGNER.contains(&format!("H\"{leaf}\"")));
    }
}
