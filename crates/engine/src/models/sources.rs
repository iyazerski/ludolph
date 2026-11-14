//! Revisions of the files that determine pi's available models.

use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use anyhow::Result;

/// File identities and write times, without keeping credentials in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sources([Option<(u64, u64, i64, i64)>; 3]);

impl Sources {
    /// Read revisions, treating absent files as unconfigured sources.
    pub(super) fn read(files: &[PathBuf; 3]) -> Result<Self> {
        let mut revisions = [None; 3];
        for (slot, path) in revisions.iter_mut().zip(files) {
            match std::fs::metadata(path) {
                Ok(meta) => *slot = Some((meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec())),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self(revisions))
    }
}
