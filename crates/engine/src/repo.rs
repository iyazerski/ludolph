//! What the composer shows about a chat's git checkout.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

/// The git checkout a folder is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// Whether it's a linked worktree rather than the repo's main one.
    pub worktree: bool,
    /// The checked-out branch; `None` on a detached `HEAD`.
    pub branch: Option<String>,
}

/// The checkout holding `cwd`, or `None` outside git.
pub async fn checkout(cwd: &Path) -> Option<Checkout> {
    let (dirs, branch) = tokio::join!(
        git(
            cwd,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
                "--git-common-dir"
            ]
        ),
        // Unlike `rev-parse HEAD`, this names a branch with no commits yet.
        git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
    );
    let dirs = dirs?;
    let mut dirs = dirs.lines();
    let worktree = dirs.next()? != dirs.next()?;
    Some(Checkout {
        worktree,
        branch: branch.map(|branch| branch.trim().to_owned()),
    })
}

/// What git printed for `args` in `cwd`, or `None` if it failed.
async fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
