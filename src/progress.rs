// progress.rs — one line per step of `dcw new`, redrawn in place (like `docker pull`).
//
// Each slow step gets its own row: a spinner or a bar while it runs, `✓` plus how long
// it took once it is done, and the finished rows stay on screen. Steps that really do
// run at the same time (the worktree checkout and the DB clone) show up side by side.
//
// Where the numbers come from:
//   - worktree checkout: dcw counts the files git has actually written into the new
//     worktree and divides by the file count `git ls-tree` reports for the commit. Both
//     are measured, not estimated.
//   - git fetch and `createdb -T`: no machine-readable progress exists (Postgres 17 has
//     no pg_stat_progress_create_* and a fresh DB reports 0 bytes until it is complete),
//     so those rows show elapsed seconds instead of a fabricated percentage.
//
// (git's own `--progress` output is not usable here: it is only emitted on a tty, and
// capturing it through a pty hangs, because a pty master never reports EOF.)

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A block of step rows on stderr. A no-op when stderr is not a terminal, so
/// piped/redirected runs print exactly what they printed before.
pub struct Progress {
    shared: Arc<Shared>,
    stopped: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

struct Task {
    label: String,
    /// Shown while the row has no percentage (e.g. "cloning template").
    note: String,
    /// Some = draw a bar; None = spin (or show `note`).
    percent: Option<u8>,
    done: bool,
    started: Instant,
    elapsed: Option<u64>,
}

struct Shared {
    tasks: Mutex<Vec<Task>>,
    /// Rows written last time, so the next redraw knows how far to move back up.
    drawn: Mutex<usize>,
    tty: bool,
}

/// One row of the block. Cheap to clone, so a worker thread can hold its own row.
#[derive(Clone)]
pub struct TaskHandle {
    idx: usize,
    shared: Arc<Shared>,
}

impl Progress {
    /// Start the block. Steps are added with `task()`.
    pub fn start() -> Progress {
        let shared = Arc::new(Shared {
            tasks: Mutex::new(Vec::new()),
            drawn: Mutex::new(0),
            tty: std::io::IsTerminal::is_terminal(&std::io::stderr()),
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let handle = shared.tty.then(|| {
            let (shared, stopped) = (shared.clone(), stopped.clone());
            std::thread::spawn(move || {
                let mut frame = 0usize;
                while !stopped.load(Ordering::Relaxed) {
                    render(&shared, frame, false);
                    frame += 1;
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
            })
        });
        Progress { shared, stopped, handle }
    }

    /// Add a row. Appended in call order, so steps started together keep the order the
    /// caller wrote them in.
    pub fn task(&self, label: &str, note: &str) -> TaskHandle {
        let mut tasks = match self.shared.tasks.lock() {
            Ok(t) => t,
            Err(_) => return TaskHandle { idx: 0, shared: self.shared.clone() },
        };
        tasks.push(Task {
            label: label.to_string(),
            note: note.to_string(),
            percent: None,
            done: false,
            started: Instant::now(),
            elapsed: None,
        });
        TaskHandle { idx: tasks.len() - 1, shared: self.shared.clone() }
    }

    /// Stop redrawing and leave the finished block on screen.
    pub fn stop(self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle {
            let _ = h.join();
        }
        render(&self.shared, 0, true);
    }
}

impl TaskHandle {
    /// The row's label (the worktree name, the step name).
    fn label(&self) -> String {
        self.shared
            .tasks
            .lock()
            .ok()
            .and_then(|t| t.get(self.idx).map(|t| t.label.clone()))
            .unwrap_or_default()
    }

    /// Say what this row is doing right now, replacing whatever it said before.
    /// Drops any bar: once the phase moves on (a percentage only ever describes the
    /// checkout), a frozen bar would hide the note. Without a draw block (stderr is not
    /// a terminal) this prints a plain line, so piped runs still report every step.
    pub fn step(&self, note: &str) {
        if self.shared.tty {
            self.shared.update(self.idx, |t| {
                t.note = note.to_string();
                t.percent = None;
            });
        } else {
            println!("  {}  {note}", self.label());
        }
    }

    /// Same as `step`, but keeps an existing note instead of replacing it (used for
    /// details like "warning: ..." that should not hide the phase in progress).
    pub fn warn(&self, note: &str) {
        if !self.shared.tty {
            eprintln!("  {}  {note}", self.label());
        }
    }

    /// Finish the row showing `verb` as its result ("✓ test-fix  aborted  6s"). Without
    /// a draw block it prints the same line plainly.
    pub fn finish(&self, verb: &str) {
        if self.shared.tty {
            self.shared.update(self.idx, |t| {
                t.done = true;
                t.note = verb.to_string();
                t.elapsed = Some(t.started.elapsed().as_secs());
            });
        } else {
            println!("  ✓ {}  {verb}", self.label());
        }
    }

    /// Mark the row finished: it freezes its elapsed time and shows `✓`. The note is
    /// dropped — it described the running state, not the result.
    pub fn done(&self) {
        self.shared.update(self.idx, |t| {
            t.done = true;
            t.note.clear();
            t.elapsed = Some(t.started.elapsed().as_secs());
        });
    }

    /// Update the bar from `done` of `total` files written. Returns false once the tree
    /// is fully on disk, so a sampler can stop; true when the total is unknown (keep
    /// sampling to the end).
    /// Update the bar from `written` of `total` files. Returns false when there is
    /// nothing left to observe (the count reached 100%, or the total is unknown), so a
    /// sampler can stop. A caller that knows when the work itself finished should drive
    /// its loop from that instead: the count can stall below 100% because git need not
    /// write exactly the files `ls-tree` lists.
    pub fn set_written(&self, written: u64, total: u64) -> bool {
        let Some(p) = percent_written(written, total) else {
            return false;
        };
        self.shared.update(self.idx, |t| t.percent = Some(p));
        p < 100
    }
}

impl Shared {
    fn update(&self, idx: usize, f: impl FnOnce(&mut Task)) {
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(t) = tasks.get_mut(idx) {
                f(t);
            }
        }
    }
}

/// Redraw the block: move back over the rows written last time, rewrite each one, and
/// clear whatever is left over from a previously longer block.
fn render(shared: &Shared, frame: usize, final_pass: bool) {
    if !shared.tty {
        return;
    }
    let Ok(tasks) = shared.tasks.lock() else { return };
    let Ok(mut drawn) = shared.drawn.lock() else { return };
    let width = tasks.iter().map(|t| t.label.chars().count()).max().unwrap_or(0);
    let mut buf = String::new();
    if *drawn > 0 {
        buf.push_str(&format!("\x1b[{}A", *drawn));
    }
    for t in tasks.iter() {
        buf.push_str("\r\x1b[K");
        buf.push_str(&line_for(t, frame, width, final_pass));
        buf.push('\n');
    }
    buf.push_str("\r\x1b[K");
    *drawn = tasks.len();
    let mut err = std::io::stderr();
    let _ = write!(err, "{buf}");
    let _ = err.flush();
}

/// One row: `  ✓ fetch origin/main  2s` once done, otherwise a spinner or a bar.
/// A step that finished inside the first second prints no time — "0s" is noise.
fn line_for(t: &Task, frame: usize, width: usize, final_pass: bool) -> String {
    let elapsed = t.elapsed.unwrap_or_else(|| t.started.elapsed().as_secs());
    let time = if elapsed == 0 { String::new() } else { format!("  {elapsed}s") };
    let label = format!("{:<width$}", t.label);
    if t.done {
        // trim_end: a row with no time would otherwise keep its label padding.
        let note = if t.note.is_empty() { String::new() } else { format!("  {}", t.note) };
        return format!("  ✓ {label}{note}{time}").trim_end().to_string();
    }
    // A frozen spinner reads as "still running"; the last draw uses a dot instead.
    let mark = if final_pass { "·" } else { FRAMES[frame % FRAMES.len()] };
    // trim_end: a row whose time is empty would otherwise keep its label padding.
    let line = match t.percent {
        Some(p) => format!("  {mark} {label}  [{}] {p}%", bar(p)),
        None if t.note.is_empty() => format!("  {mark} {label}{time}"),
        None => format!("  {mark} {label}  {}{time}", t.note),
    };
    line.trim_end().to_string()
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

    fn task(label: &str, note: &str) -> Task {
        Task {
            label: label.into(),
            note: note.into(),
            percent: None,
            done: false,
            started: Instant::now(),
            elapsed: None,
        }
    }

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
    fn rows_show_bar_note_and_check() {
        let mut t = task("checkout", "");
        t.percent = Some(50);
        let line = line_for(&t, 0, 8, false);
        assert!(line.contains("[████████████░░░░░░░░░░░░] 50%"), "{line}");

        // No percentage and no note: a spinner with the elapsed seconds.
        let fetch = task("fetch origin/main", "");
        assert!(line_for(&fetch, 0, 17, false).contains("fetch origin/main"));

        // A note stands in for the percentage.
        let db = task("database", "cloning template");
        assert!(line_for(&db, 0, 8, false).contains("cloning template"));

        // Done: the label, a check, and the frozen elapsed time.
        t.done = true;
        t.elapsed = Some(2);
        assert_eq!(line_for(&t, 0, 8, false), "  ✓ checkout  2s");

        // A step that finished inside the first second prints no time at all.
        t.elapsed = Some(0);
        assert_eq!(line_for(&t, 0, 8, false), "  ✓ checkout");

        // Final draw never leaves a spinner frozen mid-turn.
        assert!(line_for(&db, 0, 8, true).starts_with("  · database"));
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
    fn tasks_are_addressed_by_the_handle_they_returned() {
        let p = Progress::start();
        let a = p.task("fetch", "");
        let b = p.task("checkout", "");
        b.set_written(1, 2);
        b.done();
        let tasks = p.shared.tasks.lock().unwrap();
        assert!(tasks[a.idx].percent.is_none() && !tasks[a.idx].done);
        assert_eq!((tasks[b.idx].percent, tasks[b.idx].done), (Some(50), true));
        drop(tasks);
        p.stop();
    }

    // set_written drives the checkout bar. It must report "nothing left" when the count
    // reaches the total, or when the total is unknown — a sampler driven by it would
    // otherwise poll forever. (A count that stalls below the total is the caller's job:
    // `dcw new` stops on the checkout finishing, not on the percentage.)
    #[test]
    fn set_written_stops_when_there_is_nothing_left_to_measure() {
        let p = Progress::start();
        let row = p.task("checkout", "");
        assert!(row.set_written(0, 100), "start of the tree is still progress");
        assert!(row.set_written(98, 100), "a stalled count is still worth polling");
        assert!(!row.set_written(100, 100), "complete must stop the sampler");
        assert!(!row.set_written(1, 0), "an unknown total must stop the sampler");
        p.stop();
    }

    // step()/finish() are what `abort` reports through, and `abort` runs one thread per
    // worktree. The risk is index cross-talk: row N must never pick up row M's note.
    // cargo test's stderr is not a tty, so build the block with tty forced on — that is
    // the branch that stores state (the non-tty branch only prints).
    #[test]
    fn concurrent_rows_keep_their_own_state() {
        let shared = Arc::new(Shared {
            tasks: Mutex::new(Vec::new()),
            drawn: Mutex::new(0),
            tty: true,
        });
        let rows: Vec<TaskHandle> = (0..8)
            .map(|i| {
                shared.tasks.lock().unwrap().push(Task {
                    label: format!("wt{i}"),
                    note: String::new(),
                    percent: None,
                    done: false,
                    started: Instant::now(),
                    elapsed: None,
                });
                TaskHandle { idx: i, shared: shared.clone() }
            })
            .collect();
        std::thread::scope(|s| {
            for (i, row) in rows.iter().enumerate() {
                s.spawn(move || {
                    for _ in 0..50 {
                        row.step(&format!("phase {i}"));
                    }
                    row.finish("aborted");
                });
            }
        });
        let tasks = shared.tasks.lock().unwrap();
        for (i, t) in tasks.iter().enumerate() {
            assert_eq!(t.label, format!("wt{i}"));
            assert_eq!(t.note, "aborted", "row {i} picked up another row's note");
            assert!(t.done);
        }
    }

    // A finished row shows its result between the name and the time.
    #[test]
    fn finished_row_shows_its_result() {
        let mut t = task("test-fix", "");
        t.done = true;
        t.note = "aborted".into();
        t.elapsed = Some(4);
        // width is the widest label in the block; here it is this row's own label.
        assert_eq!(line_for(&t, 0, 8, false), "  ✓ test-fix  aborted  4s");
    }
}
