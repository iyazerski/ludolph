//! Watching a folder tree with FSEvents.
//!
//! FSEvents can merge and drop events, so they're hints of what changed, not
//! a log: when it says it dropped some, the watcher asks for a rescan
//! (`docs/design.md`, "Chat index"). Changes pile up in a set until the
//! engine takes them, so a burst of writes costs one pass, and memory stays
//! bounded by the number of files.
//!
//! macOS reports a write to a file when the file is closed. pi opens, appends
//! to and closes its session file for every line, so each line is seen.

use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2_core_foundation::{CFArray, CFString};
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamEventFlags,
    FSEventStreamEventId, FSEventStreamInvalidate, FSEventStreamRef, FSEventStreamRelease,
    FSEventStreamSetDispatchQueue, FSEventStreamStart, FSEventStreamStop,
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamEventFlagKernelDropped, kFSEventStreamEventFlagMustScanSubDirs,
    kFSEventStreamEventFlagUserDropped, kFSEventStreamEventIdSinceNow,
};
use tokio::sync::Notify;

/// What changed since the engine last looked.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Paths that changed, under the folder as it was given.
    pub paths: HashSet<PathBuf>,
    /// FSEvents dropped events, so anything may have changed.
    pub rescan: bool,
}

/// A running watch; dropping it stops it.
#[derive(Debug)]
pub struct Watcher {
    stream: FSEventStreamRef,
    shared: Arc<Shared>,
    // The stream calls back on this queue, so it lives as long as the stream.
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: an FSEvents stream may be stopped and released from any thread;
// its callbacks run on its own dispatch queue and only touch `Shared`.
unsafe impl Send for Watcher {}
// SAFETY: `&Watcher` only reaches `Shared`, which is `Sync`.
unsafe impl Sync for Watcher {}

#[derive(Debug)]
struct Shared {
    /// The watched folder as FSEvents reports it, with symlinks resolved,
    /// and as the caller gave it.
    reported: PathBuf,
    given: PathBuf,
    pending: Mutex<Changes>,
    notify: Notify,
}

impl Watcher {
    /// Watch the tree at `dir`, which must exist. FSEvents waits `latency`
    /// after a change for more to merge with it, except the first after a
    /// quiet spell, which comes at once.
    ///
    /// # Errors
    ///
    /// Fails if `dir` doesn't exist, or FSEvents refuses to watch it.
    pub fn start(dir: &Path, latency: Duration) -> Result<Self> {
        let reported = dir
            .canonicalize()
            .with_context(|| format!("resolving {}", dir.display()))?;
        let shared = Arc::new(Shared {
            reported: reported.clone(),
            given: dir.to_owned(),
            pending: Mutex::default(),
            notify: Notify::new(),
        });
        let path = CFString::from_str(&reported.to_string_lossy());
        let paths = CFArray::from_retained_objects(&[path]);
        let mut context = FSEventStreamContext {
            version: 0,
            info: Arc::into_raw(shared.clone()).cast_mut().cast(),
            retain: None,
            release: Some(release),
            copyDescription: None,
        };
        // SAFETY: `callback` matches FSEventStreamCallback and reads `info`
        // as the `Arc<Shared>` given here, which `release` frees once the
        // stream is done with it. FSEvents copies the context.
        let stream = unsafe {
            FSEventStreamCreate(
                None,
                Some(callback),
                &mut context,
                paths.as_opaque(),
                kFSEventStreamEventIdSinceNow,
                latency.as_secs_f64(),
                kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer,
            )
        };
        let queue = DispatchQueue::new("app.ludolph.watch", None);
        // SAFETY: `stream` was just created, and `queue` outlives it, since
        // the Watcher holds both and stops the stream first.
        let started = unsafe {
            FSEventStreamSetDispatchQueue(stream, Some(&queue));
            FSEventStreamStart(stream)
        };
        let watcher = Self {
            stream,
            shared,
            _queue: queue,
        };
        if !started {
            bail!("FSEvents can't watch {}", dir.display());
        }
        Ok(watcher)
    }

    /// Wait until something changed, and take what did.
    pub async fn changes(&self) -> Changes {
        loop {
            let notified = self.shared.notify.notified();
            let changes = std::mem::take(&mut *self.shared.pending.lock().expect("lock poisoned"));
            if changes != Changes::default() {
                return changes;
            }
            notified.await;
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: the stream is valid until released here, once. After
        // invalidating it, FSEvents calls back no more, and frees the
        // context through `release`.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
        }
    }
}

const DROPPED: u32 = kFSEventStreamEventFlagMustScanSubDirs
    | kFSEventStreamEventFlagUserDropped
    | kFSEventStreamEventFlagKernelDropped;

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<FSEventStreamEventFlags>,
    _ids: NonNull<FSEventStreamEventId>,
) {
    // SAFETY: `info` is the `Arc<Shared>` from `Watcher::start`, alive until
    // `release`. Without kFSEventStreamCreateFlagUseCFTypes, `paths` is an
    // array of `count` C strings, and `flags` has `count` entries.
    let (shared, paths, flags) = unsafe {
        (
            &*info.cast::<Shared>(),
            std::slice::from_raw_parts(paths.cast::<*const c_char>().as_ptr(), count),
            std::slice::from_raw_parts(flags.as_ptr(), count),
        )
    };
    let mut pending = shared.pending.lock().expect("lock poisoned");
    for (&path, &flag) in paths.iter().zip(flags) {
        if flag & DROPPED != 0 {
            pending.rescan = true;
            continue;
        }
        // SAFETY: FSEvents' paths are NUL-terminated.
        let path = Path::new(std::ffi::OsStr::from_bytes(
            unsafe { CStr::from_ptr(path) }.to_bytes(),
        ));
        if let Ok(rest) = path.strip_prefix(&shared.reported) {
            pending.paths.insert(shared.given.join(rest));
        }
    }
    drop(pending);
    shared.notify.notify_one();
}

unsafe extern "C-unwind" fn release(info: *const c_void) {
    // SAFETY: FSEvents calls this once, with the pointer `Watcher::start`
    // made by `Arc::into_raw`.
    drop(unsafe { Arc::from_raw(info.cast::<Shared>()) });
}
