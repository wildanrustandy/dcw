// picker.rs — arrow-key target menus for new/stop/abort/logs with no branch.
//
// Every target menu starts from ONE candidate set — the dcw-managed worktrees (children
// of $WT_DIR), each annotated with its port and up/down state — but each offers only
// what its command can act on: `stop` lists the running targets, `start` the stopped
// ones, while `restart`/`abort`/`jump` list everything. A menu whose candidates all
// fail that filter is not drawn at all (see `state_menu_rows`). Multi-select with Space,
// confirm with Enter. `new` is single-select (a shell can only cd into one), `logs`
// multi-selects log files to tail together.

use crate::config::Config;
use crate::registry;
use crate::{commands, git, process};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use std::io::Write;
use std::path::PathBuf;

struct Row {
    display: String, // pre-formatted columns
    value: String,   // the field handed back on confirm (branch name, or log path)
}

/// A dcw-managed worktree with its runtime state. `name` is the worktree's own
/// identity (its directory name) — that is what the menus show and hand back, so a
/// `git switch` inside the worktree never changes which row you pick.
struct WtRow {
    name: String,
    branch: String, // what is checked out right now (may differ from the name)
    dir: PathBuf,
    port: String,
    status: String, // "up" or "down"
}

impl WtRow {
    /// The worktree name, plus the checked-out branch when it differs — so a
    /// switched worktree is visible at a glance without losing the identity.
    fn label(&self) -> String {
        if self.branch.is_empty() || self.branch == self.name {
            self.name.clone()
        } else {
            format!("{} → {}", self.name, self.branch)
        }
    }

    /// Is this target running? True when its recorded server pid is alive or its port
    /// is bound — the same test that produces the "up"/"down" status column.
    fn up(&self) -> bool {
        self.status == "up"
    }
}

fn status_rank(s: &str) -> u8 {
    if s == "up" { 0 } else { 1 }
}

fn port_num(s: &str) -> u16 {
    s.parse().unwrap_or(u16::MAX)
}

/// Every dcw-managed worktree, sorted up-first then by port, with port + status.
/// Registry entries are looked up by WORKTREE NAME first, then by the checked-out
/// branch (older entries); a switched worktree therefore still shows its own port
/// and status. The main checkout ($REPO) is not a managed worktree and is excluded;
/// callers that should offer it add it explicitly.
fn worktree_rows(cfg: &Config) -> Vec<WtRow> {
    let reg = registry::load(&cfg.reg_file);
    // branch/name -> (port, pid)
    let mut by_key: std::collections::HashMap<String, (String, i32)> = std::collections::HashMap::new();
    for (port, info) in reg.iter() {
        if info.branch.is_empty() {
            continue;
        }
        by_key
            .entry(info.branch.clone())
            .or_insert_with(|| (port.clone(), info.pid));
    }

    let mut rows = Vec::new();
    for wt in git::list_worktrees(&cfg.repo) {
        // only dcw-managed worktrees (inside .worktrees/)
        if !git::is_managed_worktree(&wt.dir, &cfg.wt_dir) {
            continue;
        }
        let name = crate::commands::worktree_name(cfg, &wt);
        let branch = wt.branch.clone().unwrap_or_default();
        let (port, pid) = by_key
            .get(&name)
            .or_else(|| by_key.get(&branch))
            .cloned()
            .unwrap_or_else(|| ("-".into(), 0));
        let alive = pid > 0 && process::pid_alive(pid);
        let listening = port != "-" && port.parse::<u16>().map(process::port_bound).unwrap_or(false);
        let status = if alive || listening { "up" } else { "down" };
        rows.push(WtRow {
            name,
            branch,
            dir: wt.dir.clone(),
            port,
            status: status.to_string(),
        });
    }
    rows.sort_by_key(|r| (status_rank(&r.status), port_num(&r.port)));
    rows
}

fn log_rows(cfg: &Config) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let reg = registry::load(&cfg.reg_file);
    let mut slugs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for info in reg.values() {
        if info.pid > 0 && process::pid_alive(info.pid) {
            slugs.insert(cfg.slug(&info.branch), info.branch.clone());
        }
    }
    // Only RUNNING captains' logs are offered — the menu answers "which captain's
    // log do I want to watch".
    let Ok(entries) = std::fs::read_dir(&cfg.logs_dir) else {
        return rows;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".log") {
            continue;
        }
        let stem = if name.ends_with("-celery.log") {
            name[..name.len() - "-celery.log".len()].to_string()
        } else if name == "main.log" {
            cfg.slug(&cfg.base)
        } else {
            name.trim_end_matches(".log").to_string()
        };
        if let Some(branch) = slugs.get(&stem) {
            let kind = if name.ends_with("-celery.log") { "celery" } else { "server" };
            rows.push(Row {
                display: format!("{:<32} {kind}", branch),
                value: entry.path().to_string_lossy().into_owned(),
            });
        }
    }
    rows
}

/// The shared arrow-key menu. `multi = true` enables Space toggling; Enter confirms
/// the marked rows (or the row under the cursor when none are marked).
///
/// The menu draws on /dev/tty, NOT stdout: the shell function runs `jump` with its
/// stdout captured (`cd "$(...)"`), so the terminal output must not go there — only
/// the picked values do.
fn run_menu(rows: Vec<Row>, header: &str, multi: bool) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        eprintln!("dcw: no terminal available for the menu");
        return Vec::new();
    };
    let _ = enable_raw_mode();
    let _ = execute!(tty, EnterAlternateScreen, Hide);
    let result = run_menu_inner(&rows, header, multi, &mut tty);
    let _ = execute!(tty, Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}

fn draw(stdout: &mut impl Write, rows: &[Row], header: &str, cur: usize, chosen: &[bool], multi: bool) {
    // Raw mode turns OFF ONLCR, so a bare \n moves down WITHOUT returning to column 0
    // and every row drifts right by the length of the previous one. Use \r\n.
    let _ = execute!(stdout, Clear(ClearType::All), MoveTo(0, 0));
    let _ = write!(stdout, "  {header}\r\n");
    for (i, row) in rows.iter().enumerate() {
        // Checkboxes only make sense with multi-select; single-select just highlights
        // the row under the cursor (that IS the choice, confirmed by Enter).
        let line = if multi {
            let mark = if chosen[i] { "[x]" } else { "[ ]" };
            format!("{mark} {}", row.display)
        } else {
            row.display.clone()
        };
        if i == cur {
            let _ = write!(stdout, "\x1b[7m> {line}\x1b[0m\r\n");
        } else {
            let _ = write!(stdout, "  {line}\r\n");
        }
    }
    let hint = if multi {
        "  up/down move   Space select   Enter confirm   q cancel"
    } else {
        "  up/down move   Enter select   q cancel"
    };
    let _ = write!(stdout, "\r\n{hint}\r\n");
    let _ = stdout.flush();
}

fn run_menu_inner(rows: &[Row], header: &str, multi: bool, stdout: &mut impl Write) -> Vec<String> {
    let mut cur = 0usize;
    let mut chosen = vec![false; rows.len()];
    draw(stdout, rows, header, cur, &chosen, multi);
    loop {
        let key = match read() {
            Ok(Event::Key(k)) => k,
            Ok(_) => continue,       // resize/mouse/focus: keep waiting for a key
            Err(_) => return Vec::new(), // stdin closed/EOF: cancel instead of spinning
        };
        match key {
            KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. }
            | KeyEvent { code: KeyCode::Char('q'), .. }
            | KeyEvent { code: KeyCode::Char('Q'), .. }
            | KeyEvent { code: KeyCode::Esc, .. } => return Vec::new(),
            KeyEvent { code: KeyCode::Up, .. } => cur = cur.saturating_sub(1),
            KeyEvent { code: KeyCode::Down, .. } => cur = (cur + 1).min(rows.len() - 1),
            KeyEvent { code: KeyCode::Enter, .. } => {
                let picked: Vec<String> = if multi {
                    let mut v: Vec<String> = rows
                        .iter()
                        .zip(&chosen)
                        .filter(|(_, c)| **c)
                        .map(|(r, _)| r.value.clone())
                        .collect();
                    if v.is_empty() {
                        v.push(rows[cur].value.clone());
                    }
                    v
                } else {
                    vec![rows[cur].value.clone()]
                };
                return picked;
            }
            KeyEvent { code: KeyCode::Char(' '), .. } if multi => chosen[cur] = !chosen[cur],
            _ => {}
        }
        draw(stdout, rows, header, cur, &chosen, multi);
    }
}

/// One menu row per worktree. The DISPLAY shows the worktree name (identity) plus
/// the branch when it differs — the value handed back is always the name.
fn wt_menu_row(r: &WtRow) -> Row {
    Row {
        display: format!("{:<32} {:<7} {}", r.label(), r.port, r.status),
        value: r.name.clone(),
    }
}

/// The base checkout as a menu row ($REPO is not a managed worktree, so it is never in
/// `worktree_rows`; callers that should offer it insert it explicitly). Labelled
/// `(repo)` like `dcw list`, with the base branch as the value so `find_target`
/// resolves it through the base-branch path.
///
/// `branch` is left EMPTY on purpose: the column reads "name → branch when they differ",
/// and the main checkout is named for the repo, not for a branch — `(repo) → Inventory`
/// would suggest a worktree switched onto another branch, which is not what this is.
fn base_row(cfg: &Config, up: bool) -> WtRow {
    WtRow {
        name: "(repo)".into(),
        branch: String::new(),
        dir: cfg.repo.clone(),
        port: cfg.port_base.to_string(),
        status: if up { "up" } else { "down" }.to_string(),
    }
}

/// The targets a start/stop/restart menu offers, as WtRow: the managed worktrees plus
/// the base checkout when `$REPO` exists. `base_up` is passed in because probing it
/// costs an `lsof` call.
fn all_rows(cfg: &Config, base_up: bool) -> Vec<WtRow> {
    let mut rows = worktree_rows(cfg);
    if cfg.repo.is_dir() {
        rows.insert(0, base_row(cfg, base_up));
    }
    rows
}

/// Keep the targets `want` selects, as menu rows. Pure, so the selection rule is
/// testable without a terminal, a repo, or a running server.
fn menu_rows_for(rows: &[WtRow], want: impl Fn(&WtRow) -> bool) -> Vec<Row> {
    rows.iter().filter(|r| want(r)).map(wt_menu_row).collect()
}

/// Menu rows for a state-filtered command (`start`/`stop`/`restart`): every target that
/// `want` selects, in the shared menu shape.
///
/// `Err(code)` means there is no menu to draw, and the caller should exit with `code`.
/// The two reasons are checked in this order, and the order is the point:
///
/// 1. **An empty selection is the command's work already being done** — `dcw stop` with
///    every captain down, `dcw start` with every captain up. That is a successful no-op,
///    so it never becomes a menu and never becomes an error: `empty_note` picks how it
///    reads, `None` for exit 0 with NO output at all (the `stop`/`start` contract),
///    `Some(msg)` for a message and exit 1 (a command where "no targets" means the
///    environment is wrong, not that the work is done). Deciding this BEFORE the
///    terminal check is what makes `dcw stop` idempotent in a script: with nothing to
///    stop it exits 0 quietly instead of demanding a tty.
/// 2. **A missing terminal is a usage error** (`2`). There are targets to pick from and
///    no way to pick, which no target state can fix.
fn state_menu_rows(
    cfg: &Config,
    cmd: &str,
    want: impl Fn(&WtRow) -> bool,
    empty_note: Option<&str>,
) -> Result<Vec<Row>, i32> {
    let base_up = base_running(cfg);
    let rows = menu_rows_for(&all_rows(cfg, base_up), want);
    if rows.is_empty() {
        return Err(match empty_note {
            Some(note) => {
                eprintln!("{note}");
                1
            }
            None => 0,
        });
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!("usage: dcw {cmd} <target>   (run it in a terminal to pick from a list)");
        return Err(2);
    }
    Ok(rows)
}

/// jump: pick a single worktree (up or dead) and print its directory. The base
/// branch is included as the repo root, so `dcw jump` can also take you home.
pub fn jump_pick(cfg: &Config) -> i32 {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!("usage: dcw jump   (run it in a terminal to pick from a list)");
        return 2;
    }
    let mut rows: Vec<Row> = worktree_rows(cfg)
        .iter()
        .map(|r| Row {
            display: format!("{:<32} {:<7} {}", r.label(), r.port, r.status),
            value: r.dir.to_string_lossy().into_owned(),
        })
        .collect();

    // Base branch lives in $REPO itself; show it first so "go to the main checkout"
    // is always one Enter away.
    if cfg.repo.is_dir() {
        rows.insert(
            0,
            Row {
                display: format!("{:<32} {:<7} {}", cfg.base, "-", "base"),
                value: cfg.repo.to_string_lossy().into_owned(),
            },
        );
    }

    if rows.is_empty() {
        eprintln!(
            "no worktrees under {} — nothing to jump to",
            cfg.wt_dir.display()
        );
        return 1;
    }
    let picked = run_menu(rows, "WORKTREE  PORT  STATUS", false);
    let Some(dir) = picked.into_iter().next() else {
        return 0; // cancelled
    };
    // stdout carries ONLY the path (captured by the shell function).
    println!("{dir}");

    // If stdout is a terminal, nobody captured it — the `dcw()` shell function is
    // not active in this session (old terminal, or rc not reloaded), so the path
    // was just printed instead of cd'ing. Say so instead of leaving the user
    // wondering why `pwd` did not change.
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        eprintln!();
        eprintln!("note: shell integration is not active in this terminal, so dcw could");
        eprintln!("      not cd for you. Run `exec $SHELL` (or open a new terminal) and");
        eprintln!("      try again; check with: type dcw   (should say: shell function)");
    }
    0
}

/// True when the base checkout's server is running (pid alive per registry, or port bound).
fn base_running(cfg: &Config) -> bool {
    let reg = registry::load(&cfg.reg_file);
    if let Some(info) = reg.get(&cfg.port_base.to_string()) {
        if info.pid > 0 && process::pid_alive(info.pid) {
            return true;
        }
    }
    process::port_bound(cfg.port_base)
}

/// The targets `dcw start` offers: everything not currently running.
fn startable(r: &WtRow) -> bool {
    !r.up()
}

/// The targets `dcw stop` offers: only what is currently running.
fn stoppable(r: &WtRow) -> bool {
    r.up()
}

/// start: pick a stopped target and start it. Only targets that are DOWN are offered
/// (managed worktrees and the base checkout, which runs in $REPO itself). When every
/// target is already up there is nothing to start, so the command exits silently.
pub fn start_pick(cfg: &Config) -> i32 {
    let rows = match state_menu_rows(cfg, "start", startable, None) {
        Ok(rows) => rows,
        Err(code) => return code,
    };
    let picked = run_menu(rows, "WORKTREE  PORT  STATUS", false);
    let Some(name) = picked.into_iter().next() else {
        return 0;
    };
    commands::start(cfg, Some(name))
}

/// stop: multi-select targets to stop. Only targets that are UP are offered — stopping
/// something already down is a no-op, and dead rows only make the live ones harder to
/// find. When every target is down the command exits silently.
pub fn stop_pick(cfg: &Config) -> i32 {
    let rows = match state_menu_rows(cfg, "stop", stoppable, None) {
        Ok(rows) => rows,
        Err(code) => return code,
    };
    let picked = run_menu(rows, "WORKTREE  PORT  STATUS", true);
    for name in picked {
        commands::stop(cfg, Some(name));
    }
    0
}

/// restart: multi-select targets to stop then start (managed worktrees and the base
/// checkout). Restarting a down target is meaningful (it starts it), so every target is
/// offered; only "no targets at all" is worth a message.
pub fn restart_pick(cfg: &Config) -> i32 {
    let rows = match state_menu_rows(cfg, "restart", |_| true, Some("no targets — nothing to restart")) {
        Ok(rows) => rows,
        Err(code) => return code,
    };
    let picked = run_menu(rows, "WORKTREE  PORT  STATUS", true);
    for name in picked {
        commands::restart(cfg, Some(name));
    }
    0
}

/// abort: multi-select worktrees to destroy (with a combined confirmation).
pub fn abort_pick(cfg: &Config) -> i32 {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!("usage: dcw abort <worktree>   (run it in a terminal to pick from a list)");
        return 2;
    }
    let rows = worktree_rows(cfg);
    if rows.is_empty() {
        eprintln!(
            "no worktrees under {} — nothing to abort",
            cfg.wt_dir.display()
        );
        return 1;
    }
    let menu: Vec<Row> = rows.iter().map(wt_menu_row).collect();
    let picked = run_menu(menu, "WORKTREE  PORT  STATUS", true);
    if picked.is_empty() {
        return 0;
    }

    // Combined summary + one confirmation (dcw's multi-target abort).
    let targets: Vec<(String, String)> = picked
        .iter()
        .map(|name| {
            let p = commands::port_of(cfg, name)
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into());
            (name.clone(), p)
        })
        .collect();
    commands::print_destroy_summary(cfg, &targets);

    use std::io::Write;
    print!("Proceed? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err()
        || !matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
    {
        println!("aborted");
        return 1;
    }
    println!();
    let resolved: Vec<(String, std::path::PathBuf, Option<u16>)> = picked
        .iter()
        .filter_map(|name| {
            let Some(t) = commands::find_target(cfg, name) else {
                eprintln!("dcw: {name} disappeared — skipped");
                return None;
            };
            let port = commands::port_for(cfg, &t);
            Some((t.name, t.dir.clone(), port))
        })
        .collect();

    // One row per worktree, aborted at the same time: each target's slow step is
    // `dropdb`, and nothing in an abort depends on another target. Output goes through
    // each target's own row, so the interleaved plain prints of a sequential loop are
    // gone. A single target still gets its own block (with one row).
    let progress = crate::progress::Progress::start();
    let rows: Vec<_> = resolved.iter().map(|(name, _, _)| progress.task(name, "")).collect();
    std::thread::scope(|s| {
        for ((name, dir, port), row) in resolved.iter().zip(&rows) {
            s.spawn(move || commands::abort_one(cfg, name, dir, *port, row));
        }
    });
    progress.stop();
    0
}

/// logs: multi-select over running captains' logs; tail them together.
pub fn logs_pick(cfg: &Config) -> i32 {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!("usage: dcw logs <worktree> [celery]   (run it in a terminal to pick from a list)");
        return 2;
    }
    let rows = log_rows(cfg);
    if rows.is_empty() {
        eprintln!("no running captains — nothing to tail (start one with 'dcw new')");
        return 1;
    }
    let picked = run_menu(rows, "LOG", true);
    if picked.is_empty() {
        return 0;
    }
    let mut cmd = std::process::Command::new("tail");
    cmd.arg("-f");
    for p in &picked {
        cmd.arg(p);
    }
    use std::os::unix::process::CommandExt;
    let _ = cmd.exec();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, port: &str, up: bool) -> WtRow {
        WtRow {
            name: name.into(),
            branch: name.into(),
            dir: PathBuf::from(format!("/wt/{name}")),
            port: port.into(),
            status: if up { "up" } else { "down" }.into(),
        }
    }

    /// `stop` must offer only running targets: a down worktree has nothing to stop, and
    /// offering it just buries the live rows.
    #[test]
    fn stop_offers_only_running_targets() {
        let all = [row("a", "5201", true), row("b", "5202", false), row("c", "5203", true)];
        let rows = menu_rows_for(&all, stoppable);
        let names: Vec<&str> = rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
    }

    /// `start` is the mirror: only stopped targets are offerable.
    #[test]
    fn start_offers_only_stopped_targets() {
        let all = [row("a", "5201", true), row("b", "5202", false), row("c", "5203", false)];
        let rows = menu_rows_for(&all, startable);
        let names: Vec<&str> = rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(names, ["b", "c"]);
    }

    /// All-down `stop` / all-up `start` leaves no row to draw — which the caller turns
    /// into a silent success, not an empty menu.
    #[test]
    fn filters_can_empty_the_menu() {
        assert!(menu_rows_for(&[row("a", "5201", false)], stoppable).is_empty());
        assert!(menu_rows_for(&[row("a", "5201", true)], startable).is_empty());
    }

    /// `restart` restarts anything (a down target is just started), so it never filters.
    #[test]
    fn restart_offers_every_target() {
        let all = [row("a", "5201", true), row("b", "5202", false)];
        assert_eq!(menu_rows_for(&all, |_| true).len(), 2);
    }

    /// Menu displays keep the "name port status" shape the header promises.
    #[test]
    fn menu_row_shows_port_and_status() {
        let rows = menu_rows_for(&[row("a", "5201", true)], stoppable);
        assert_eq!(rows[0].display, format!("{:<32} {:<7} {}", "a", "5201", "up"));
    }

    /// A switched worktree shows its branch, but a row with no branch (what `base_row`
    /// produces for the main checkout) must NOT read as "switched": `(repo) → Inventory`
    /// would suggest a worktree moved onto another branch, which is not what $REPO is.
    #[test]
    fn a_row_without_a_branch_keeps_its_bare_label() {
        let repo_row = WtRow {
            name: "(repo)".into(),
            branch: String::new(),
            dir: PathBuf::from("/repo"),
            port: "5200".into(),
            status: "down".into(),
        };
        assert_eq!(repo_row.label(), "(repo)");
        assert_eq!(row("a", "5201", true).label(), "a");

        let switched = WtRow {
            name: "PRD_INV_1".into(),
            branch: "fix/login".into(),
            ..row("PRD_INV_1", "5201", true)
        };
        assert_eq!(switched.label(), "PRD_INV_1 → fix/login");
    }
}