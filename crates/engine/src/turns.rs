//! What each run changed in its repo (`docs/design.md`, "Turn changes").
//!
//! A run's snapshots are git trees, written with a copy of the repo's index
//! into an object store of the run's own, with the repo's objects as an
//! alternate. So the repo gets no new objects and its index stays as it
//! is. Once the run's diff is saved, its store goes.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// How long one snapshot may take before the run's changes are skipped.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest changed file a snapshot takes. A bigger one skips the run,
/// since hashing it would be slow and its diff unreadable.
const FILE_LIMIT: u64 = 10 * 1024 * 1024;
/// The longest patch kept for a run.
const PATCH_LIMIT: usize = 1024 * 1024;
/// The most the saved runs may take on disk; the oldest go first.
const BUDGET: u64 = 50 * 1024 * 1024;

/// One run's changes, as the phone's diff page reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub patch: String,
    pub files: Vec<File>,
    pub additions: u64,
    pub deletions: u64,
    /// The patch stops at its limit, so it leaves files or lines out.
    pub truncated: bool,
}

/// A file a run changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
}

/// A run's size, for the button under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
}

impl From<&Diff> for Counts {
    fn from(changes: &Diff) -> Self {
        Self {
            files: changes.files.len(),
            additions: changes.additions,
            deletions: changes.deletions,
        }
    }
}

/// The working tree before a run, in the run's own object store, which
/// goes when this is dropped.
#[derive(Debug)]
pub struct Snapshot {
    repo: Repo,
    store: PathBuf,
    tree: String,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.store)
            && e.kind() != ErrorKind::NotFound
        {
            eprintln!("removing {}: {e}", self.store.display());
        }
    }
}

#[derive(Debug, Clone)]
struct Repo {
    /// The working tree's top folder.
    top: PathBuf,
    /// The repo's own index and object store.
    index: PathBuf,
    objects: PathBuf,
}

/// The saved runs: `<dir>/<session id>/<key>.json`, where the key is the
/// timestamp of the user message that started the run.
#[derive(Debug)]
pub struct Turns {
    dir: PathBuf,
    /// By session id, so a chat view doesn't parse whole patches.
    counts: Mutex<HashMap<String, BTreeMap<i64, Counts>>>,
}

impl Turns {
    /// Keep runs under `dir`, and drop the stores of runs a stopped engine
    /// left behind.
    ///
    /// # Errors
    ///
    /// Fails if the leftover stores can't be removed.
    pub fn new(dir: PathBuf) -> Result<Self> {
        let runs = dir.join("runs");
        match std::fs::remove_dir_all(&runs) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("removing {}", runs.display()));
            }
            _ => {}
        }
        Ok(Self {
            dir,
            counts: Mutex::default(),
        })
    }

    /// Snapshot the working tree of the repo `cwd` is in, before a run.
    /// Returns `None` if it isn't in a git repo.
    ///
    /// # Errors
    ///
    /// Fails if git fails, takes too long, or a changed file is too big.
    pub async fn before(&self, cwd: &Path) -> Result<Option<Snapshot>> {
        let Some(repo) = repo(cwd).await else {
            return Ok(None);
        };
        let store = self.dir.join("runs").join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(store.join("objects"))
            .with_context(|| format!("creating {}", store.display()))?;
        let mut snapshot = Snapshot {
            repo,
            store,
            tree: String::new(),
        };
        // A repo with no commits yet has no index.
        match std::fs::copy(&snapshot.repo.index, snapshot.store.join("index")) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                return Err(e).context("copying the repo's index");
            }
            _ => {}
        }
        snapshot.tree = tree(&snapshot).await?;
        Ok(Some(snapshot))
    }

    /// Snapshot the working tree again after the run, and return what
    /// changed since `before`.
    ///
    /// # Errors
    ///
    /// Fails if git fails, takes too long, or a changed file is too big.
    pub async fn after(&self, before: Snapshot) -> Result<Diff> {
        let after = tree(&before).await?;
        compare(&before, &before.tree, &after).await
    }

    /// Diff the current staged, unstaged and untracked files against HEAD.
    /// Use a private index and object store, leaving the checkout untouched.
    ///
    /// # Errors
    ///
    /// Fails under the same git, size and time limits as a run snapshot.
    pub async fn working(&self, cwd: &Path) -> Result<Option<Diff>> {
        let Some(snapshot) = self.before(cwd).await? else {
            return Ok(None);
        };
        let base = match git(
            &snapshot,
            &["rev-parse", "--verify", "--quiet", "HEAD^{tree}"],
        )
        .await
        {
            Ok(head) => head,
            // A new repository has no HEAD yet.
            Err(_) => git(&snapshot, &["hash-object", "-w", "-t", "tree", "--stdin"]).await?,
        };
        compare(&snapshot, base.trim(), &snapshot.tree)
            .await
            .map(Some)
    }

    /// Save `changes` as session `session`'s run `key`, then drop the oldest
    /// runs past the budget.
    ///
    /// # Errors
    ///
    /// Fails if the run can't be written.
    pub fn save(&self, session: &str, key: i64, changes: &Diff) -> Result<()> {
        let path = self.path(session, key)?;
        let dir = path
            .parent()
            .expect("a run's file is in its session's folder");
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        ludolph_json_file::save(&path, changes)?;
        self.lock()
            .entry(session.to_owned())
            .or_default()
            .insert(key, Counts::from(changes));
        if let Err(e) = self.prune() {
            eprintln!("pruning saved runs: {e:#}");
        }
        Ok(())
    }

    /// Session `session`'s run `key`, if it was saved.
    ///
    /// # Errors
    ///
    /// Fails if the run can't be read.
    pub fn load(&self, session: &str, key: i64) -> Result<Option<Diff>> {
        ludolph_json_file::load(&self.path(session, key)?)
    }

    /// The sizes of session `session`'s saved runs, by key.
    ///
    /// # Errors
    ///
    /// Fails if they can't be read.
    pub fn counts(&self, session: &str) -> Result<BTreeMap<i64, Counts>> {
        if let Some(counts) = self.lock().get(session) {
            return Ok(counts.clone());
        }
        let dir = self.dir.join(checked(session)?);
        let mut counts = BTreeMap::new();
        for (key, path) in runs(&dir)? {
            if let Some(changes) = ludolph_json_file::load::<Diff>(&path)? {
                counts.insert(key, Counts::from(&changes));
            }
        }
        self.lock().insert(session.to_owned(), counts.clone());
        Ok(counts)
    }

    /// Drop session `session`'s saved runs, as its chat is deleted.
    ///
    /// # Errors
    ///
    /// Fails if they can't be removed.
    pub fn forget(&self, session: &str) -> Result<()> {
        let dir = self.dir.join(checked(session)?);
        self.lock().remove(session);
        match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", dir.display()))
            }
            _ => Ok(()),
        }
    }

    /// Drop the oldest saved runs until they fit the budget.
    fn prune(&self) -> Result<()> {
        let mut saved: Vec<(SystemTime, u64, String, i64, PathBuf)> = Vec::new();
        for session in read_dir(&self.dir)? {
            let Some(name) = session
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if name == "runs" || !session.is_dir() {
                continue;
            }
            for (key, path) in runs(&session)? {
                let meta = std::fs::metadata(&path)?;
                saved.push((meta.modified()?, meta.len(), name.clone(), key, path));
            }
        }
        let mut total: u64 = saved.iter().map(|run| run.1).sum();
        saved.sort_by_key(|run| run.0);
        for (_, len, session, key, path) in saved {
            if total <= BUDGET {
                break;
            }
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            if let Some(counts) = self.lock().get_mut(&session) {
                counts.remove(&key);
            }
            total -= len;
        }
        Ok(())
    }

    fn path(&self, session: &str, key: i64) -> Result<PathBuf> {
        Ok(self.dir.join(checked(session)?).join(format!("{key}.json")))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, BTreeMap<i64, Counts>>> {
        self.counts.lock().expect("lock poisoned")
    }
}

/// `session`, if it's safe as a folder name. pi's ids are UUIDs.
fn checked(session: &str) -> Result<&str> {
    let safe = !session.is_empty()
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe {
        bail!("{session:?} isn't a session id");
    }
    Ok(session)
}

/// The saved runs in a session's folder, by key.
fn runs(dir: &Path) -> Result<Vec<(i64, PathBuf)>> {
    let files = read_dir(dir)?.into_iter().filter_map(|path| {
        let key = path
            .file_name()?
            .to_str()?
            .strip_suffix(".json")?
            .parse()
            .ok()?;
        Some((key, path))
    });
    Ok(files.collect())
}

/// The entries of `dir`, or none if it doesn't exist.
fn read_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    entries
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("reading {}", dir.display()))
}

/// Read one snapshot range as the same capped diff used for saved runs.
async fn compare(snapshot: &Snapshot, before: &str, after: &str) -> Result<Diff> {
    let range = [before, after];
    let numstat = git(
        snapshot,
        &[
            &["diff-tree", "-r", "--no-renames", "--numstat", "-z"][..],
            &range,
        ]
        .concat(),
    )
    .await?;
    let mut files = Vec::new();
    let (mut additions, mut deletions) = (0, 0);
    // `<added>\t<deleted>\t<path>\0`, with `-` for a binary file's counts.
    for record in numstat.split('\0').filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            bail!("git diff-tree printed {record:?}");
        };
        additions += added.parse::<u64>().unwrap_or(0);
        deletions += deleted.parse::<u64>().unwrap_or(0);
        files.push(File {
            path: path.to_owned(),
        });
    }
    let patch_args = [
        "diff-tree",
        "-r",
        "-p",
        "--no-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ];
    let mut patch = git(snapshot, &[&patch_args[..], &range].concat()).await?;
    let truncated = patch.len() > PATCH_LIMIT;
    if truncated {
        // At a line's end, so the patch still parses.
        let mut end = PATCH_LIMIT;
        while !patch.is_char_boundary(end) {
            end -= 1;
        }
        end = patch[..end].rfind('\n').map_or(0, |at| at + 1);
        patch.truncate(end);
    }
    Ok(Diff {
        patch,
        files,
        additions,
        deletions,
        truncated,
    })
}

/// The repo whose working tree holds `cwd`, if any.
async fn repo(cwd: &Path) -> Option<Repo> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "rev-parse",
            "--show-toplevel",
            "--git-path",
            "index",
            "--git-path",
            "objects",
        ])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let out = String::from_utf8(out.stdout).ok()?;
    let mut lines = out.lines();
    let top = PathBuf::from(lines.next()?);
    // `--git-path` answers relative to `cwd` unless it's absolute.
    let index = cwd.join(lines.next()?);
    let objects = cwd.join(lines.next()?);
    Some(Repo {
        top,
        index,
        objects,
    })
}

/// Write the working tree as a tree in the snapshot's store, and return its
/// id.
async fn tree(snapshot: &Snapshot) -> Result<String> {
    let write = async {
        too_big(snapshot).await?;
        git(snapshot, &["add", "-A"]).await?;
        Ok::<_, anyhow::Error>(git(snapshot, &["write-tree"]).await?.trim().to_owned())
    };
    tokio::time::timeout(SNAPSHOT_TIMEOUT, write)
        .await
        .with_context(|| format!("the snapshot took over {}s", SNAPSHOT_TIMEOUT.as_secs()))?
}

/// Fail if a file the snapshot would take is over [`FILE_LIMIT`].
async fn too_big(snapshot: &Snapshot) -> Result<()> {
    let changed = git(
        snapshot,
        &[
            "ls-files",
            "-z",
            "--modified",
            "--others",
            "--exclude-standard",
        ],
    )
    .await?;
    for path in changed.split('\0').filter(|p| !p.is_empty()) {
        let len = std::fs::metadata(snapshot.repo.top.join(path)).map_or(0, |meta| meta.len());
        if len > FILE_LIMIT {
            bail!("{path} is over {} MB", FILE_LIMIT / 1024 / 1024);
        }
    }
    Ok(())
}

/// Run git in the snapshot's repo, against the snapshot's index and store,
/// and return what it printed.
async fn git(snapshot: &Snapshot, args: &[&str]) -> Result<String> {
    let alternates: OsString = snapshot.repo.objects.clone().into_os_string();
    let out = Command::new("git")
        // The user's fsmonitor hook would run, and quoting would mangle paths.
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false"])
        .args(args)
        .current_dir(&snapshot.repo.top)
        .env("GIT_INDEX_FILE", snapshot.store.join("index"))
        .env("GIT_OBJECT_DIRECTORY", snapshot.store.join("objects"))
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", alternates)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .context("running git")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    // Files in other encodings show with replacement characters.
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
