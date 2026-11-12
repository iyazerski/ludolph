//! The pi processes the engine runs, saved to a file so that an engine
//! restarted after a crash can stop the ones left behind before it starts
//! new ones on the same sessions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// How often [`Runtime::reap`] checks whether a leftover process exited.
const POLL: Duration = Duration::from_millis(50);

/// The engine's pi processes, by session id.
#[derive(Debug)]
pub struct Runtime {
    path: PathBuf,
    processes: BTreeMap<String, Process>,
}

/// One pi process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Process {
    pid: u32,
    /// When the process started, in microseconds since the epoch, so a
    /// reused pid is never mistaken for it.
    started: u64,
    /// Whether a run was going.
    running: bool,
}

impl Runtime {
    /// Stop the pi processes a previous engine left in the file at `path`,
    /// and start an empty runtime there.
    ///
    /// Each leftover gets SIGTERM, which lets it finish writing its session,
    /// and its process group gets SIGKILL if it's still running after
    /// `grace`. Returns the sessions whose runs were cut short.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read or written.
    pub async fn reap(path: &Path, grace: Duration) -> Result<(Self, Vec<String>)> {
        let leftovers: BTreeMap<String, Process> =
            ludolph_json_file::load(path)?.unwrap_or_default();
        let alive: Vec<&Process> = leftovers
            .values()
            .filter(|process| start_time(process.pid) == Some(process.started))
            .collect();
        for process in &alive {
            signal(process.pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + grace;
        while alive
            .iter()
            .any(|process| start_time(process.pid) == Some(process.started))
            && Instant::now() < deadline
        {
            tokio::time::sleep(POLL).await;
        }
        for process in &alive {
            if start_time(process.pid) == Some(process.started) {
                kill_group(process.pid);
            }
        }
        let interrupted = leftovers
            .iter()
            .filter(|(_, process)| process.running)
            .map(|(session, _)| session.clone())
            .collect();
        let runtime = Self {
            path: path.to_owned(),
            processes: BTreeMap::new(),
        };
        runtime.save()?;
        Ok((runtime, interrupted))
    }

    /// Record that `pid` is the pi process for `session`.
    ///
    /// # Errors
    ///
    /// Fails if the process is gone already, or the file can't be written.
    pub fn add(&mut self, session: &str, pid: u32) -> Result<()> {
        let started = start_time(pid).ok_or_else(|| anyhow::anyhow!("pi {pid} exited"))?;
        let process = Process {
            pid,
            started,
            running: false,
        };
        self.processes.insert(session.to_owned(), process);
        self.save()
    }

    /// Record whether `session`'s process has a run going.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be written.
    pub fn set_running(&mut self, session: &str, running: bool) -> Result<()> {
        match self.processes.get_mut(session) {
            Some(process) if process.running != running => process.running = running,
            _ => return Ok(()),
        }
        self.save()
    }

    /// Forget `session`'s process, once it has exited.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be written.
    pub fn remove(&mut self, session: &str) -> Result<()> {
        if self.processes.remove(session).is_none() {
            return Ok(());
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        ludolph_json_file::save(&self.path, &self.processes)
    }
}

/// When process `pid` started, in microseconds since the epoch; `None` if
/// there's no such process.
fn start_time(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: `info` is a writable buffer of `size` bytes, which is what
    // PROC_PIDTBSDINFO fills.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole struct, and it was zeroed before.
    let info = unsafe { info.assume_init() };
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// Send `signal` to process `pid`. A process that's gone is fine.
fn signal(pid: u32, signal: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: kill only takes plain integers.
        unsafe { libc::kill(pid, signal) };
    }
}

/// Kill the process group that process `pid` leads, with the tools it
/// started. A group that's gone is fine.
pub(crate) fn kill_group(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: killpg only takes plain integers.
        unsafe { libc::killpg(pid, libc::SIGKILL) };
    }
}
