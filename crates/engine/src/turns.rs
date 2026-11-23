//! What each run changed, kept per chat so its diffs outlive commits and later edits.
//!
//! Snapshots read the repository through a private index and object store, so they never change
//! its state. A run copies the blobs it changed into its chat's own object store.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// Skip oversized files rather than hashing them on the send path.
const FILE_LIMIT: u64 = 10 * 1024 * 1024;
const PATCH_LIMIT: usize = 1024 * 1024;
/// The most the saved chats may take on disk; the least recently changed go first.
const BUDGET: u64 = 100 * 1024 * 1024;
/// `git diff-tree --raw`'s mode for a side where the path doesn't exist.
const ABSENT: &str = "000000";
/// A submodule's commit, which isn't in the store.
const GITLINK: &str = "160000";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
}

/// What one run changed, saved as `<session id>/<key>.json`, with its blobs in `<session id>/git`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Run {
    changes: BTreeMap<String, Change>,
    additions: u64,
    deletions: u64,
}

impl From<&Run> for Counts {
    fn from(run: &Run) -> Self {
        Self {
            files: run.changes.len(),
            additions: run.additions,
            deletions: run.deletions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Change {
    before: Option<Entry>,
    after: Option<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    mode: String,
    oid: String,
}

#[derive(Debug)]
struct Scratch(PathBuf);

impl Scratch {
    fn new(runs: &Path) -> Result<Self> {
        let dir = runs.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(dir.join("objects"))
            .with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0)
            && e.kind() != ErrorKind::NotFound
        {
            eprintln!("removing {}: {e}", self.0.display());
        }
    }
}

/// The working tree as a run starts, in a private object store that lasts until the run is saved.
#[derive(Debug)]
pub struct Snapshot {
    repo: Repo,
    store: Scratch,
    tree: String,
}

impl Snapshot {
    fn git(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        // The user's fsmonitor hook would run, and quoting would mangle paths.
        command
            .args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false"])
            .args(args)
            .current_dir(&self.repo.top)
            .env("GIT_INDEX_FILE", self.store.0.join("index"))
            .env("GIT_OBJECT_DIRECTORY", self.store.0.join("objects"))
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &self.repo.objects)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE");
        command
    }
}

#[derive(Debug, Clone)]
struct Repo {
    top: PathBuf,
    index: PathBuf,
    objects: PathBuf,
}

#[derive(Debug, Default)]
struct Cache {
    /// By session id, so a chat view doesn't parse every run.
    runs: HashMap<String, BTreeMap<i64, Counts>>,
    chats: HashMap<String, Counts>,
    /// Bumped by every save, so counts read meanwhile aren't cached.
    saves: u64,
}

#[derive(Debug)]
pub struct Turns {
    dir: PathBuf,
    cache: Mutex<Cache>,
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
            cache: Mutex::default(),
        })
    }

    pub async fn before(&self, cwd: &Path) -> Result<Option<Snapshot>> {
        let Some(repo) = repo(cwd).await else {
            return Ok(None);
        };
        let mut snapshot = Snapshot {
            repo,
            store: Scratch::new(&self.dir.join("runs"))?,
            tree: String::new(),
        };
        snapshot.tree = tree(&snapshot).await?;
        Ok(Some(snapshot))
    }

    pub async fn after(&self, session: &str, before: Snapshot) -> Result<Run> {
        let after = tree(&before).await?;
        let raw = run(
            before.git(&["diff-tree", "-r", "-z", "--no-renames", "--raw"]),
            &[&before.tree, &after],
            None,
        )
        .await?;
        let changes = changes(&raw)?;
        let (_, additions, deletions) =
            numstat(|args| before.git(args), &before.tree, &after).await?;
        if !changes.is_empty() {
            let store = self.store(session)?;
            if !store.join("HEAD").exists() {
                let mut init = Command::new("git");
                init.args(["init", "--bare", "--quiet", "--template="])
                    .arg(&store)
                    .env_remove("GIT_DIR")
                    .env_remove("GIT_WORK_TREE");
                run(init, &[], None).await?;
            }
            let blobs: BTreeSet<&str> = changes
                .values()
                .flat_map(|change| [&change.before, &change.after])
                .flatten()
                .filter(|entry| entry.mode != GITLINK)
                .map(|entry| entry.oid.as_str())
                .collect();
            let list: String = blobs.iter().map(|oid| format!("{oid}\n")).collect();
            let mut pack = before.git(&["pack-objects", "--quiet"]);
            pack.arg(store.join("objects/pack/pack"));
            run(pack, &[], Some(list.into_bytes())).await?;
        }
        Ok(Run {
            changes,
            additions,
            deletions,
        })
    }

    pub fn save(&self, session: &str, key: i64, run: &Run) -> Result<()> {
        let path = self.path(session, key)?;
        let dir = path
            .parent()
            .expect("a run's file is in its session's folder");
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        ludolph_json_file::save(&path, run)?;
        {
            let mut cache = self.lock();
            cache.saves += 1;
            cache.chats.remove(session);
            // Unloaded counts stay unloaded, so the next read finds every run.
            if let Some(runs) = cache.runs.get_mut(session) {
                runs.insert(key, Counts::from(run));
            }
        }
        if let Err(e) = self.prune(session) {
            eprintln!("pruning saved runs: {e:#}");
        }
        Ok(())
    }

    pub async fn load(&self, session: &str, key: i64) -> Result<Option<Diff>> {
        let Some(run) = ludolph_json_file::load::<Run>(&self.path(session, key)?)? else {
            return Ok(None);
        };
        self.diff(session, &run.changes).await.map(Some)
    }

    /// Every run's changes in one diff: each path from before its first run to after its last.
    pub async fn chat(&self, session: &str) -> Result<Diff> {
        let changes = net(self.runs(session)?);
        self.diff(session, &changes).await
    }

    pub async fn chat_counts(&self, session: &str) -> Result<Counts> {
        let saves = {
            let cache = self.lock();
            if let Some(counts) = cache.chats.get(session) {
                return Ok(*counts);
            }
            cache.saves
        };
        let changes = net(self.runs(session)?);
        let counts = if changes.is_empty() {
            Counts::default()
        } else {
            let (scratch, before, after) = self.trees(session, &changes).await?;
            let store = self.store(session)?;
            let (files, additions, deletions) =
                numstat(|args| stored(&store, &scratch, args), &before, &after).await?;
            Counts {
                files: files.len(),
                additions,
                deletions,
            }
        };
        let mut cache = self.lock();
        if cache.saves == saves {
            cache.chats.insert(session.to_owned(), counts);
        }
        Ok(counts)
    }

    pub fn counts(&self, session: &str) -> Result<BTreeMap<i64, Counts>> {
        let saves = {
            let cache = self.lock();
            if let Some(counts) = cache.runs.get(session) {
                return Ok(counts.clone());
            }
            cache.saves
        };
        let mut counts = BTreeMap::new();
        for (key, path) in runs(&self.dir.join(checked(session)?))? {
            if let Some(run) = ludolph_json_file::load::<Run>(&path)? {
                counts.insert(key, Counts::from(&run));
            }
        }
        let mut cache = self.lock();
        if cache.saves == saves {
            cache.runs.insert(session.to_owned(), counts.clone());
        }
        Ok(counts)
    }

    pub fn forget(&self, session: &str) -> Result<()> {
        let dir = self.dir.join(checked(session)?);
        self.uncache(session);
        match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", dir.display()))
            }
            _ => Ok(()),
        }
    }

    /// Drop whole chats, since their runs share one store, but never the one just saved.
    fn prune(&self, keep: &str) -> Result<()> {
        let mut saved: Vec<(SystemTime, u64, String, PathBuf)> = Vec::new();
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
            let (len, modified) = usage(&session)?;
            saved.push((modified, len, name, session));
        }
        let mut total: u64 = saved.iter().map(|chat| chat.1).sum();
        saved.sort_by_key(|chat| chat.0);
        for (_, len, session, dir) in saved {
            if total <= BUDGET {
                break;
            }
            if session == keep {
                continue;
            }
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
            self.uncache(&session);
            total -= len;
        }
        Ok(())
    }

    fn runs(&self, session: &str) -> Result<Vec<Run>> {
        let mut paths = runs(&self.dir.join(checked(session)?))?;
        paths.sort_by_key(|(key, _)| *key);
        let mut out = Vec::new();
        for (_, path) in paths {
            out.extend(ludolph_json_file::load::<Run>(&path)?);
        }
        Ok(out)
    }

    async fn diff(&self, session: &str, changes: &BTreeMap<String, Change>) -> Result<Diff> {
        if changes.is_empty() {
            return Ok(Diff::default());
        }
        let (scratch, before, after) = self.trees(session, changes).await?;
        let store = self.store(session)?;
        compare(|args| stored(&store, &scratch, args), &before, &after).await
    }

    /// Trees of just the changed paths, written to a scratch store over the chat's.
    async fn trees(
        &self,
        session: &str,
        changes: &BTreeMap<String, Change>,
    ) -> Result<(Scratch, String, String)> {
        let store = self.store(session)?;
        let scratch = Scratch::new(&self.dir.join("runs"))?;
        let before = changes
            .iter()
            .filter_map(|(p, c)| Some((p, c.before.as_ref()?)));
        let before = write_tree(&store, &scratch, "before", before).await?;
        let after = changes
            .iter()
            .filter_map(|(p, c)| Some((p, c.after.as_ref()?)));
        let after = write_tree(&store, &scratch, "after", after).await?;
        Ok((scratch, before, after))
    }

    fn store(&self, session: &str) -> Result<PathBuf> {
        Ok(self.dir.join(checked(session)?).join("git"))
    }

    fn path(&self, session: &str, key: i64) -> Result<PathBuf> {
        Ok(self.dir.join(checked(session)?).join(format!("{key}.json")))
    }

    fn uncache(&self, session: &str) {
        let mut cache = self.lock();
        cache.runs.remove(session);
        cache.chats.remove(session);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().expect("lock poisoned")
    }
}

fn stored(store: &Path, scratch: &Scratch, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.quotePath=false"])
        .args(args)
        .current_dir(store)
        .env("GIT_DIR", store)
        .env("GIT_OBJECT_DIRECTORY", scratch.0.join("objects"))
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", store.join("objects"))
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    command
}

async fn write_tree<'a>(
    store: &Path,
    scratch: &Scratch,
    index: &str,
    entries: impl Iterator<Item = (&'a String, &'a Entry)>,
) -> Result<String> {
    let mut info = Vec::new();
    for (path, entry) in entries {
        info.extend(format!("{} {}\t{path}\0", entry.mode, entry.oid).into_bytes());
    }
    let index = scratch.0.join(index);
    let mut update = stored(store, scratch, &["update-index", "-z", "--index-info"]);
    update.env("GIT_INDEX_FILE", &index);
    run(update, &[], Some(info)).await?;
    let mut write = stored(store, scratch, &["write-tree", "--missing-ok"]);
    write.env("GIT_INDEX_FILE", &index);
    Ok(run(write, &[], None).await?.trim().to_owned())
}

/// Fold runs in order, so a path goes from its first run's before to its last run's after.
fn net(runs: Vec<Run>) -> BTreeMap<String, Change> {
    let mut net: BTreeMap<String, Change> = BTreeMap::new();
    for run in runs {
        for (path, change) in run.changes {
            match net.get_mut(&path) {
                Some(kept) => kept.after = change.after,
                None => {
                    net.insert(path, change);
                }
            }
        }
    }
    net.retain(|_, change| change.before != change.after);
    net
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

fn usage(dir: &Path) -> Result<(u64, SystemTime)> {
    let (mut len, mut modified) = (0, SystemTime::UNIX_EPOCH);
    let mut dirs = vec![dir.to_owned()];
    while let Some(dir) = dirs.pop() {
        for path in read_dir(&dir)? {
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                dirs.push(path);
            } else {
                len += meta.len();
                modified = modified.max(meta.modified()?);
            }
        }
    }
    Ok((len, modified))
}

fn changes(raw: &str) -> Result<BTreeMap<String, Change>> {
    let side = |mode: &str, oid: &str| {
        (mode != ABSENT).then(|| Entry {
            mode: mode.to_owned(),
            oid: oid.to_owned(),
        })
    };
    let mut fields = raw.split('\0').filter(|f| !f.is_empty());
    let mut out = BTreeMap::new();
    while let Some(meta) = fields.next() {
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let (Some(path), [before_mode, after_mode, before, after, _]) = (fields.next(), &parts[..])
        else {
            bail!("git diff-tree printed {meta:?}");
        };
        out.insert(
            path.to_owned(),
            Change {
                before: side(before_mode, before),
                after: side(after_mode, after),
            },
        );
    }
    Ok(out)
}

async fn compare(git: impl Fn(&[&str]) -> Command, before: &str, after: &str) -> Result<Diff> {
    let (files, additions, deletions) = numstat(&git, before, after).await?;
    let patch_args = [
        "diff-tree",
        "-r",
        "-p",
        "--no-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ];
    let mut patch = run(git(&patch_args), &[before, after], None).await?;
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

async fn numstat(
    git: impl Fn(&[&str]) -> Command,
    before: &str,
    after: &str,
) -> Result<(Vec<File>, u64, u64)> {
    let numstat = run(
        git(&["diff-tree", "-r", "--no-renames", "--numstat", "-z"]),
        &[before, after],
        None,
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

/// The repo's index with every changed path added, as a tree.
///
/// Only the paths `git status` names are hashed, so a large repository costs about one status.
async fn tree(snapshot: &Snapshot) -> Result<String> {
    let write = async {
        // A fresh copy each time, so the after tree has what a run committed.
        let index = snapshot.store.0.join("index");
        match std::fs::copy(&snapshot.repo.index, &index) {
            Ok(_) => {}
            // A repo with no commits yet has no index.
            Err(e) if e.kind() == ErrorKind::NotFound => match std::fs::remove_file(&index) {
                Err(e) if e.kind() != ErrorKind::NotFound => {
                    return Err(e).context("removing the snapshot's index");
                }
                _ => {}
            },
            Err(e) => return Err(e).context("copying the repo's index"),
        }
        let paths = dirty(&snapshot.repo.top).await?;
        if !paths.is_empty() {
            let list: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain([0])).collect();
            let add = snapshot.git(&[
                "add",
                "--all",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ]);
            run(add, &[], Some(list)).await?;
        }
        Ok::<_, anyhow::Error>(
            run(snapshot.git(&["write-tree"]), &[], None)
                .await?
                .trim()
                .to_owned(),
        )
    };
    tokio::time::timeout(SNAPSHOT_TIMEOUT, write)
        .await
        .with_context(|| format!("the snapshot took over {}s", SNAPSHOT_TIMEOUT.as_secs()))?
}

/// Paths that differ from `HEAD` or the index, and untracked ones `.gitignore` allows.
async fn dirty(top: &Path) -> Result<Vec<String>> {
    // The repo's own index, so its fsmonitor and untracked cache help; no locks, so it's not written.
    let mut status = Command::new("git");
    status
        .args([
            "--no-optional-locks",
            "-c",
            "core.quotePath=false",
            "status",
        ])
        .args(["--porcelain", "-z", "--no-renames", "--untracked-files=all"])
        .current_dir(top)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    let out = run(status, &[], None).await?;
    let mut paths = Vec::new();
    for record in out.split('\0').filter(|r| !r.is_empty()) {
        let (Some(code), Some(path)) = (record.get(..2), record.get(3..)) else {
            bail!("git status printed {record:?}");
        };
        // A nested repository shows as its folder.
        if path.ends_with('/') {
            continue;
        }
        let meta = std::fs::symlink_metadata(top.join(path));
        if let Ok(meta) = &meta
            && meta.len() > FILE_LIMIT
        {
            bail!("{path} is over {} MB", FILE_LIMIT / 1024 / 1024);
        }
        // Gone from the index and the worktree, so the copied index has it right already.
        if meta.is_err() && !code.ends_with('D') {
            continue;
        }
        paths.push(path.to_owned());
    }
    paths.sort_unstable();
    paths.dedup();
    Ok(paths)
}

async fn run(mut command: Command, args: &[&str], input: Option<Vec<u8>>) -> Result<String> {
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("running git")?;
    let stdin = child.stdin.take();
    let write = async {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            stdin.write_all(&input).await?;
        }
        Ok::<_, std::io::Error>(())
    };
    let (written, out) = tokio::join!(write, child.wait_with_output());
    let out = out.context("running git")?;
    if !out.status.success() {
        let args: Vec<_> = command.as_std().get_args().collect();
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {args:?} failed: {}", stderr.trim());
    }
    written.context("writing to git")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
