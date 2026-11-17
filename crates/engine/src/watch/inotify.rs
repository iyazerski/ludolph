use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::event::CreateKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::Notify;

use super::Changes;

#[derive(Debug)]
pub struct Watcher {
    _inotify: RecommendedWatcher,
    shared: Arc<Shared>,
    latency: Duration,
}

#[derive(Debug, Default)]
struct Shared {
    pending: Mutex<Changes>,
    notify: Notify,
}

impl Watcher {
    pub fn start(dir: &Path, latency: Duration) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let events = shared.clone();
        let mut inotify =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // The engine's own reads open files; they change nothing.
                if let Ok(event) = &event
                    && event.kind.is_access()
                {
                    return;
                }
                let mut pending = events.pending.lock().expect("lock poisoned");
                match event {
                    // Files written into a new folder before its watch begins send no events.
                    Ok(event)
                        if event.need_rescan()
                            || event.kind == EventKind::Create(CreateKind::Folder) =>
                    {
                        pending.rescan = true;
                    }
                    Ok(event) => pending.paths.extend(event.paths),
                    Err(e) => {
                        eprintln!("watching pi's folder: {e}");
                        pending.rescan = true;
                    }
                }
                drop(pending);
                events.notify.notify_one();
            })
            .context("starting inotify")?;
        inotify
            .watch(dir, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", dir.display()))?;
        Ok(Self {
            _inotify: inotify,
            shared,
            latency,
        })
    }

    /// Waits `latency` after the first event, so a burst of writes comes as one change,
    /// as FSEvents batches them on macOS.
    pub async fn changes(&self) -> Changes {
        loop {
            let notified = self.shared.notify.notified();
            if *self.shared.pending.lock().expect("lock poisoned") == Changes::default() {
                notified.await;
                tokio::time::sleep(self.latency).await;
            }
            let changes = std::mem::take(&mut *self.shared.pending.lock().expect("lock poisoned"));
            if changes != Changes::default() {
                return changes;
            }
        }
    }
}
