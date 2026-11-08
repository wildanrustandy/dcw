// git.rs — git worktree / branch operations via the git CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Run git with args and return (success, stdout). Stderr is captured for messages.
pub fn git_output(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output();
    match out {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(_) => (false, String::new(), "git not found".to_string()),
    }
}

pub fn git_status(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The branch currently checked out in `repo`.
pub fn current_branch(repo: &Path) -> Option<String> {
    let (ok, out, _) = git_output(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if ok {
        let s = out.trim().to_string();
        if !s.is_empty() && s != "HEAD" {
            return Some(s);
        }
    }
    None
}

/// Does a local branch or remote ref exist (respectively)?
pub fn ref_exists(repo: &Path, full_ref: &str) -> bool {
    git_status(repo, &["show-ref", "--verify", "--quiet", full_ref])
}

pub fn fetch_origin(repo: &Path) -> (bool, String) {
    let (ok, out, err) = git_output(repo, &["fetch", "origin"]);
    let msg = if err.trim().is_empty() { out } else { err };
    (ok, msg)
}

/// Add a worktree for an existing branch.
pub fn worktree_add_existing(repo: &Path, wt: &Path, branch: &str) -> bool {
    git_status(repo, &["worktree", "add", wt.to_str().unwrap_or(""), branch])
}

/// Add a worktree that creates a NEW branch starting at a ref, untracked.
pub fn worktree_add_new(repo: &Path, wt: &Path, branch: &str, start: &str) -> bool {
    git_status(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            wt.to_str().unwrap_or(""),
            "--no-track",
            start,
        ],
    )
}

/// Remove a worktree by force (dcw: `git worktree remove --force`).
pub fn worktree_remove(repo: &Path, wt: &Path) -> bool {
    git_status(repo, &["worktree", "remove", "--force", wt.to_str().unwrap_or("")])
}

pub fn branch_delete(repo: &Path, branch: &str) {
    // branch may not exist; `-D` is force. Ignore the result.
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "-D", branch])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// A parsed porcelain entry for `git worktree list --porcelain`.
#[derive(Default, Clone)]
pub struct Worktree {
    pub dir: PathBuf,
    pub branch: Option<String>, // None = detached (agent scratch worktrees)
    pub head: String,
}

pub fn list_worktrees(repo: &Path) -> Vec<Worktree> {
    let (ok, out, _) = git_output(repo, &["worktree", "list", "--porcelain"]);
    if !ok {
        return Vec::new();
    }
    let mut result = Vec::new();
    for block in out.split("\n\n") {
        let mut wt = Worktree::default();
        let mut has = false;
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("worktree ") {
                wt.dir = PathBuf::from(v);
                has = true;
            } else if let Some(v) = line.strip_prefix("branch ") {
                wt.branch = Some(v.trim_start_matches("refs/heads/").to_string());
            } else if let Some(v) = line.strip_prefix("HEAD ") {
                wt.head = v.chars().take(9).collect();
            }
        }
        if has {
            result.push(wt);
        }
    }
    result
}

/// True if `dir` is inside `wt_dir` (a dcw-managed worktree, not the main repo or an
/// external worktree). Prefix check, not dirname: 'feat/x' lives one level deeper.
pub fn is_managed_worktree(dir: &Path, wt_dir: &Path) -> bool {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let wt = wt_dir.canonicalize().unwrap_or_else(|_| wt_dir.to_path_buf());
    dir.starts_with(&wt)
}