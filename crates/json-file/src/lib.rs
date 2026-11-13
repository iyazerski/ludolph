//! Small JSON state files, read whole and replaced whole and atomically:
//! the engine's index and settings, and the relay's state.

use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Read and parse the file at `path`; `None` if there is none.
pub fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `value` to `path` atomically, readable only by the user (some of
/// these files hold secrets).
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    save_bytes(path, &serde_json::to_vec_pretty(value)?)
}

/// Write `bytes` to `path` atomically, readable only by the user.
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
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn files_round_trip_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        assert_eq!(load::<BTreeMap<String, u32>>(&path).unwrap(), None);
        let value = BTreeMap::from([("a".to_owned(), 1)]);
        save(&path, &value).unwrap();
        assert_eq!(load(&path).unwrap(), Some(value));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::write(&path, "not json").unwrap();
        assert!(load::<BTreeMap<String, u32>>(&path).is_err());
    }
}
