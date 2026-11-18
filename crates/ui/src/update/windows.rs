use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use super::{REPO, curl, signed};

const KEY: &[u8] = include_bytes!("../../../../assets/update-public-key.der");
const ASSET: &str = "Ludolph-Setup.exe";

pub fn install(version: &str) -> Result<PathBuf> {
    ensure!(
        super::newer(version, crate::VERSION)?,
        "the requested update isn't newer than this app"
    );
    let base = format!("https://github.com/{REPO}/releases/download/v{version}");
    let body = curl(&[
        "--max-filesize",
        "4096",
        &format!("{base}/windows-update.json"),
    ])?;
    let signature = curl(&[
        "--max-filesize",
        "1024",
        &format!("{base}/windows-update.sig"),
    ])?;
    let manifest = signed::verify(KEY, &body, &signature, version)?;
    let updates =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA isn't set")?)
            .join("Ludolph/updates");
    std::fs::create_dir_all(&updates)?;
    let temporary = tempfile::NamedTempFile::new_in(&updates)?.into_temp_path();
    let destination = temporary
        .to_str()
        .context("the update path isn't Unicode")?;
    curl(&[
        "--max-filesize",
        "268435456",
        "--output",
        destination,
        &format!("{base}/{ASSET}"),
    ])?;
    ensure!(
        signed::checksum(std::fs::File::open(&temporary)?)? == manifest.sha256.to_ascii_lowercase(),
        "the installer doesn't match the signed update"
    );
    let installer = updates.join(ASSET);
    temporary
        .persist(&installer)
        .context("saving the verified installer")?;
    Ok(installer)
}

pub fn relaunch(installer: &Path) -> Result<()> {
    // Inno Setup replaces the Windows app and starts it again. On launch it syncs the bundled
    // engine only in the WSL distribution the user approved during setup.
    Command::new(installer)
        .args(["/SILENT", "/NORESTART", "/CLOSEAPPLICATIONS", "/UPDATE=1"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("starting the verified installer")?;
    Ok(())
}
