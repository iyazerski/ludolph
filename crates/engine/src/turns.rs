//! Snapshot with a private index and object store; never change the user's repository state.

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

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// Skip oversized files rather than hashing them on the send path.
const FILE_LIMIT: u64 = 10 * 1024 * 1024;
const PATCH_LIMIT: usize = 1024 * 1024;
/// The most the saved runs may take on disk; the oldest go first.
const BUDGET: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub patch: String,
    pub files: Vec<File>,
    pub additions: u64,
    pub deletions: u64,
    /// The patch stops at its limit, so it leaves files or lines out.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
}

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

/// A private object store, deleted on drop after the run's diff has been saved.
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
    top: PathBuf,
    index: PathBuf,
    objects: PathBuf,
}

/// `<session id>/<user-message timestamp>.json`.
#[derive(Debug)]
pub struct Turns {
    dir: PathBuf,
    /// By session id, so a chat view doesn't parse whole patches.
    counts: Mutex<HashMap<String, BTreeMap<i64, Counts>>>,
}

impl Turns {
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

    pub async fn after(&self, before: Snapshot) -> Result<Diff> {
        let after = tree(&before).await?;
        compare(&before, &before.tree, &after).await
    }

    /// Read staged, unstaged and untracked changes through a private index and object store.
    pub async fn working(&self, cwd: &Path) -> Result<Option<Diff>> {
        let Some((snapshot, base)) = self.working_tree(cwd).await? else {
            return Ok(None);
        };
        compare(&snapshot, &base, &snapshot.tree).await.map(Some)
    }

    /// [`Self::working`] without the patch, which can be large and only the Changes view shows.
    pub async fn working_counts(&self, cwd: &Path) -> Result<Option<Counts>> {
        let Some((snapshot, base)) = self.working_tree(cwd).await? else {
            return Ok(None);
        };
        let (files, additions, deletions) = numstat(&snapshot, &base, &snapshot.tree).await?;
        Ok(Some(Counts {
            files: files.len(),
            additions,
            deletions,
        }))
    }

    async fn working_tree(&self, cwd: &Path) -> Result<Option<(Snapshot, String)>> {
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
        let base = base.trim().to_owned();
        Ok(Some((snapshot, base)))
    }

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

    pub fn load(&self, session: &str, key: i64) -> Result<Option<Diff>> {
        ludolph_json_file::load(&self.path(session, key)?)
    }

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

async fn compare(snapshot: &Snapshot, before: &str, after: &str) -> Result<Diff> {
    let (files, additions, deletions) = numstat(snapshot, before, after).await?;
    let range = [before, after];
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
        // Truncate at a line boundary so the partial patch still parses.
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

async fn numstat(snapshot: &Snapshot, before: &str, after: &str) -> Result<(Vec<File>, u64, u64)> {
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
    // Numstat is NUL-separated; binary files report '-' instead of line counts.
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
    Ok((files, additions, deletions))
}

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
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
