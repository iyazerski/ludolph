#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(
            || PathBuf::from(path),
            |home| PathBuf::from(home).join(rest),
        ),
        None => PathBuf::from(path),
    }
}

/// The path in WSL of a folder picked on Windows, from `wslpath` in the distribution the engine
/// runs in, which knows where drives are mounted. A folder in another distribution isn't there.
#[cfg(windows)]
pub async fn to_wsl(path: &Path) -> Result<String, String> {
    let picked = path
        .to_str()
        .ok_or_else(|| format!("{} isn't valid Unicode", path.display()))?;
    let script = r#"printf '%s\n' "$WSL_DISTRO_NAME" && wslpath -a -u "$1""#;
    let out = tokio::process::Command::new("wsl.exe")
        .args(["--exec", "sh", "-c", script, "sh", picked])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .await
        .map_err(|e| format!("running wsl.exe: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let (Some(distribution), Some(wsl), true) = (lines.next(), lines.next(), out.status.success())
    else {
        return Err(format!("WSL can't reach {picked}"));
    };
    if let Some(other) = distribution_of(picked)
        && !other.eq_ignore_ascii_case(distribution)
    {
        return Err(format!(
            "The folder is in {other}, but Ludolph runs in {distribution}"
        ));
    }
    Ok(wsl.to_owned())
}

/// The WSL distribution whose files a Windows path is in: `\\wsl.localhost\<name>\...`.
#[cfg(windows)]
fn distribution_of(path: &str) -> Option<&str> {
    let lower = path.to_ascii_lowercase();
    let prefix = [r"\\wsl.localhost\", r"\\wsl$\"]
        .into_iter()
        .find(|prefix| lower.starts_with(prefix))?;
    path[prefix.len()..].split('\\').next()
}

#[cfg(all(test, windows))]
mod tests {
    use super::distribution_of;

    #[test]
    fn finds_the_distribution_of_wsl_folders() {
        let cases = [
            (r"\\wsl.localhost\Ubuntu\home\me", Some("Ubuntu")),
            (r"\\WSL$\Debian", Some("Debian")),
            (r"C:\Users\me\code", None),
        ];
        for (picked, distribution) in cases {
            assert_eq!(distribution_of(picked), distribution, "for {picked}");
        }
    }
}
