use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Atomically replace the file with mode 0600; callers may store secrets.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    save_bytes(path, &serde_json::to_vec_pretty(value)?)
}

pub fn save_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    // Per process and write, so two runs (say, `--install` and the agent),
    // or two tasks in one, can't share it.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp.{}.{write}", std::process::id()));
    // A fresh file, so the mode applies: a leftover one keeps its own.
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing {}", tmp.display()));
        }
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // On Windows only the app writes here, in the user's own AppData.
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}
