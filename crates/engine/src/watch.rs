use std::collections::HashSet;
use std::path::PathBuf;

#[cfg_attr(target_os = "macos", path = "watch/fsevents.rs")]
#[cfg_attr(target_os = "linux", path = "watch/inotify.rs")]
mod backend;

pub use backend::Watcher;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub paths: HashSet<PathBuf>,
    /// The system dropped events, so anything may have changed.
    pub rescan: bool,
}
