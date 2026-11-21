// progress.rs — the progress line for the silent waits in `dcw new`.
//
// The slow steps print nothing until they finish, which reads as a hang. This module
// draws one line on stderr that either spins (no known progress) or fills a bar from a
// real percentage.
//
// Where the numbers come from:
//   - worktree checkout: dcw counts the files git has actually written into the new
//     worktree and divides by the file count `git ls-tree` reports for the commit. Both
//     are measured, not estimated.
//   - git fetch and `createdb -T`: no machine-readable progress exists (Postgres 17 has
//     no pg_stat_progress_create_* and a fresh DB reports 0 bytes until it is complete),
//     so those steps show elapsed seconds instead of a fabricated percentage.
//
// (git's own `--progress` output is not usable here: it is only emitted on a tty, and
// capturing it through a pty hangs, because a pty master never reports EOF.)

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

/// A progress line on stderr. Stopping it clears the line.
pub struct Progress {
    stop: Arc<AtomicBool>,
    /// Percentage 0-100, or 101 for "unknown" (spin instead of a bar).
    percent: Arc<AtomicU8>,
    handle: Option<std::thread::JoinHandle<()>>,
}

const UNKNOWN: u8 = 101;

impl Progress {
    /// Start a line labelled `label` with no known progress (spins). A no-op when
    /// stderr is not a terminal, so piped/redirected runs are unchanged.
    pub fn start(label: &str) -> Progress {
        let tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let percent = Arc::new(AtomicU8::new(UNKNOWN));
        let stop = Arc::new(AtomicBool::new(false));
        let label = label.to_string();
        let handle = tty.then(|| {
            let (stop, percent) = (stop.clone(), percent.clone());
            std::thread::spawn(move || {
                let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let start = std::time::Instant::now();
                let mut i = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let p = percent.load(Ordering::Relaxed);
                    let secs = start.elapsed().as_secs();
                    let line = if p <= 100 {
                        format!("  {} [{}] {p}% ({secs}s)", frames[i % frames.len()], bar(p))
                    } else {
                        format!("  {} {label} ({secs}s)", frames[i % frames.len()])
                    };
                    let _ = write!(std::io::stderr(), "\r\x1b[K{line}");
                    let _ = std::io::stderr().flush();
                    i += 1;
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
            })
        });
        Progress { stop, percent, handle }
    }

    /// Set a real percentage (clamped to 100). Stays a bar from here on.
    pub fn set_percent(&self, percent: u8) {
        self.percent.store(percent.min(100), Ordering::Relaxed);
    }

    /// Update the bar from `done` of `total` files written. Returns false once the tree
    /// is fully on disk (or the count is unknown), so a sampler can stop.
    pub fn set_written(&self, done: u64, total: u64) -> bool {
        match percent_written(done, total) {
            Some(p) => {
                self.set_percent(p);
                p < 100
            }
            None => true, // unknown total: keep sampling to the end
        }
    }

    /// Clear the line and stop.
    pub fn stop(self) {
        self.shutdown();
    }

    fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if self.handle.is_some() {
            // The draw thread notices the flag within one 120ms frame.
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = write!(std::io::stderr(), "\r\x1b[K");
            let _ = std::io::stderr().flush();
        }
    }
}

/// A 24-cell bar for a 0-100 percentage.
fn bar(percent: u8) -> String {
    const WIDTH: usize = 24;
    let filled = (percent as usize * WIDTH) / 100;
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

/// How many files `rev` checks out, so a caller can turn "files written so far" into a
/// percentage. 0 means unknown (e.g. git could not read the tree).
pub fn checkout_file_count(repo: &std::path::Path, rev: &str) -> u64 {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-tree", "-r", "--name-only", rev])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().count() as u64,
        _ => 0,
    }
}

/// Count the regular files currently under `dir`, skipping `.git` (the worktree's gitdir
/// file and anything nested). Used only for progress: a miss under-reports, never
/// over-reports.
pub fn files_written(dir: &std::path::Path) -> u64 {
    fn walk(dir: &std::path::Path, n: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if entry.file_name() != ".git" {
                    walk(&path, n);
                }
            } else {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

/// Percentage from how much of the tree has landed on disk, or None if unknown.
pub fn percent_written(done: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some(((done.min(total) * 100) / total) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_fills_proportionally() {
        assert_eq!(bar(0), "░".repeat(24));
        assert_eq!(bar(100), "█".repeat(24));
        assert_eq!(bar(50).chars().filter(|c| *c == '█').count(), 12);
    }

    #[test]
    fn percent_never_exceeds_100_and_handles_empty() {
        assert_eq!(percent_written(0, 10), Some(0));
        assert_eq!(percent_written(5, 10), Some(50));
        assert_eq!(percent_written(20, 10), Some(100)); // over-count clamps
        assert_eq!(percent_written(3, 0), None);
    }

    #[test]
    fn counts_files_but_not_git() {
        let dir = std::env::temp_dir().join(format!("dcw-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a"), b"x").unwrap();
        std::fs::write(dir.join("sub/b"), b"x").unwrap();
        std::fs::write(dir.join(".git/c"), b"x").unwrap();
        assert_eq!(files_written(&dir), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_then_stop_returns() {
        let p = Progress::start("testing");
        p.set_percent(50);
        std::thread::sleep(std::time::Duration::from_millis(150));
        p.stop();
    }
}