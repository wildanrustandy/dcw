// commands.rs — the subcommand handlers and the port/server lifecycle.

use crate::config::Config;
use crate::registry::{self, Lock, Registry};
use crate::{celery, db, git, process};
use std::path::{Path, PathBuf};

/// Look up the port recorded for a branch (empty if none).
pub fn port_of(cfg: &Config, branch: &str) -> Option<u16> {
    let reg = registry::load(&cfg.reg_file);
    for (port, info) in reg.iter() {
        if info.branch == branch {
            if let Ok(p) = port.parse::<u16>() {
                return Some(p);
            }
        }
    }
    None
}

/// The live server pid recorded for a port (None if dead/absent).
fn pid_for_port(cfg: &Config, port: u16) -> Option<i32> {
    let reg = registry::load(&cfg.reg_file);
    let key = port.to_string();
    if let Some(info) = reg.get(&key) {
        if info.pid > 0 && process::pid_alive(info.pid) {
            return Some(info.pid);
        }
    }
    None
}

fn record_pid(cfg: &Config, port: u16, branch: &str, pid: i32) {
    let mut reg = registry::load(&cfg.reg_file);
    let key = port.to_string();
    let started_at = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
    );
    let entry = reg.entry(key).or_default();
    if entry.branch.is_empty() {
        entry.branch = branch.to_string();
    }
    entry.pid = pid;
    entry.started_at = started_at;
    registry::save(&cfg.reg_file, &reg);
}

/// Release a port registration (by port or branch).
fn port_free(cfg: &Config, key: &str) {
    let _lock = Lock::acquire(cfg.lock_dir.clone());
    let reg = registry::load(&cfg.reg_file);
    let mut out = Registry::new();
    for (port, info) in reg.iter() {
        if port == key || info.branch == key {
            continue;
        }
        out.insert(port.clone(), info.clone());
    }
    registry::save(&cfg.reg_file, &out);
}

/// Allocate the first free port in [PORT_BASE+1, PORT_MAX-1] for a branch, claiming it.
fn port_alloc(cfg: &Config, branch: &str) -> Result<u16, String> {
    let _lock = Lock::acquire(cfg.lock_dir.clone());
    if let Some(p) = port_of(cfg, branch) {
        return Ok(p);
    }
    let mut reg = registry::load(&cfg.reg_file);
    for p in (cfg.port_base + 1)..cfg.port_max {
        let key = p.to_string();
        if reg.contains_key(&key) {
            continue;
        }
        if process::port_bound(p) {
            continue;
        }
        let info = registry::PortInfo {
            branch: branch.to_string(),
            pid: 0,
            started_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
            ),
        };
        reg.insert(key, info);
        registry::save(&cfg.reg_file, &reg);
        return Ok(p);
    }
    Err(format!("no free port in {}-{}", cfg.port_base + 1, cfg.port_max - 1))
}

/// Kill everything bound to a port and clear its registry pid (base branch keeps a
/// pid=0 tombstone so its port stays reserved).
fn kill_port(cfg: &Config, port: u16) {
    // Kill the recorded parent pid first (a reloader parent may respawn a killed
    // child).
    let rec = registry::load(&cfg.reg_file)
        .get(&port.to_string())
        .and_then(|i| (i.pid > 0).then_some(i.pid));
    if let Some(pid) = rec {
        process::signal(pid, nix::sys::signal::Signal::SIGTERM);
    }
    for pid in lsof_listeners(port) {
        process::signal(pid, nix::sys::signal::Signal::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    if let Some(pid) = rec {
        process::signal(pid, nix::sys::signal::Signal::SIGKILL);
    }
    for pid in lsof_listeners(port) {
        process::signal(pid, nix::sys::signal::Signal::SIGKILL);
    }
    // clear pid (keep the entry for the base branch; drop the entry otherwise).
    let mut reg = registry::load(&cfg.reg_file);
    let key = port.to_string();
    if let Some(info) = reg.get_mut(&key) {
        if info.branch == cfg.base {
            info.pid = 0;
        } else {
            reg.remove(&key);
        }
    }
    registry::save(&cfg.reg_file, &reg);
}

/// PIDs listening on a TCP port via lsof.
fn lsof_listeners(port: u16) -> Vec<i32> {
    let out = std::process::Command::new("lsof")
        .args([&format!("-tiTCP:{port}"), "-sTCP:LISTEN"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .filter_map(|s| s.parse::<i32>().ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// Locate manage.py in a tree (dcw `_find_django_root`).
fn find_django_root(tree: &Path) -> Option<PathBuf> {
    for cand in [".", "hmx", "src", "backend", "app"] {
        let candidate = tree.join(cand).join("manage.py");
        if candidate.is_file() {
            return Some(tree.join(cand));
        }
    }
    None
}

/// Start the runserver in `dir` on port, tracking pid. Returns pid or None.
fn start_server(cfg: &Config, tree: &Path, port: u16, branch: &str) -> Option<i32> {
    let Some(root) = find_django_root(tree) else {
        eprintln!("⚠  manage.py not found in {} (or common subdirs) — server NOT started", tree.display());
        return None;
    };
    let log = server_log(cfg, branch);

    // DevSpace-style: prefer $VENV/bin/hmx, then `hmx` on PATH, else manage.py.
    let (program, args): (String, Vec<String>) = if let Some(hmx) = process::venv_bin(&cfg.venv, "hmx") {
        (
            hmx.to_string_lossy().into_owned(),
            vec!["runserver".to_string(), format!("127.0.0.1:{port}")],
        )
    } else if process::which("hmx").is_some() {
        (
            "hmx".to_string(),
            vec!["runserver".to_string(), format!("127.0.0.1:{port}")],
        )
    } else if let Some(py) = process::venv_bin(&cfg.venv, "python3").or_else(|| process::which("python3")) {
        (
            py.to_string_lossy().into_owned(),
            vec!["manage.py".to_string(), "runserver".to_string(), format!("127.0.0.1:{port}"), "--noreload".to_string()],
        )
    } else {
        eprintln!("⚠  no server command (hmx/python3) found — server NOT started");
        return None;
    };

    let pid = process::spawn_detached(&root, &log, &program, &args).ok()?;
    record_pid(cfg, port, branch, pid);
    Some(pid)
}

pub fn server_log(cfg: &Config, branch: &str) -> PathBuf {
    if branch == cfg.base {
        cfg.logs_dir.join("main.log")
    } else {
        cfg.logs_dir.join(format!("{}.log", cfg.slug(branch)))
    }
}

fn ensure_dirs(cfg: &Config) {
    let _ = std::fs::create_dir_all(&cfg.logs_dir);
    let _ = std::fs::create_dir_all(&cfg.celery_state_dir);
}

/// ── new ──────────────────────────────────────────────────────────
/// Create a NEW worktree (clone DB, copy private files, set Redis namespace) and
/// bring it up. Errors if the worktree already exists — running an existing one is
/// `dcw start`'s job.
pub fn create(cfg: &Config, branch: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let branch = match branch {
        Some(b) => b,
        None => {
            eprintln!("usage: dcw new <branch>   (create a worktree; 'dcw start' runs an existing one)");
            return 2;
        }
    };
    if !cfg.repo.join(".git").exists() {
        eprintln!("error: {} is not a git repo — clone first", cfg.repo.display());
        return 2;
    }

    if branch == cfg.base {
        eprintln!("error: the base branch runs in {} itself, not in a worktree.", cfg.repo.display());
        eprintln!("       use 'dcw start {branch}' to run it.");
        return 2;
    }

    let wt = cfg.dir_for(&branch);
    if wt.exists() {
        eprintln!("error: worktree for {branch} already exists — use 'dcw start {branch}' to run it");
        return 2;
    }

    if let Some(head) = git::current_branch(&cfg.repo) {
        if head == branch {
            eprintln!("error: {branch} is checked out in {} — git cannot add it as a worktree.", cfg.repo.display());
            eprintln!("       switch {} to {}, or set DEV_CAPTAIN_BASE={branch} if that is your base branch.", cfg.repo.display(), cfg.base);
            return 2;
        }
    }

    // fetch so a new branch starts at the latest remote commit
    let (ok, msg) = git::fetch_origin(&cfg.repo);
    if !ok {
        eprintln!("dcw: warning: git fetch origin failed; using existing local refs");
    } else if !msg.trim().is_empty() {
        for line in msg.lines().rev().take(3).collect::<Vec<_>>().iter().rev() {
            println!("{line}");
        }
    }

    // start-point order: existing local branch → origin/<base> → local <base>
    let added = if git::ref_exists(&cfg.repo, &format!("refs/heads/{branch}")) {
        git::worktree_add_existing(&cfg.repo, &wt, &branch)
    } else if git::ref_exists(&cfg.repo, &format!("refs/remotes/origin/{}", cfg.base)) {
        git::worktree_add_new(&cfg.repo, &wt, &branch, &format!("origin/{}", cfg.base))
    } else if git::ref_exists(&cfg.repo, &format!("refs/heads/{}", cfg.base)) {
        git::worktree_add_new(&cfg.repo, &wt, &branch, &cfg.base)
    } else {
        eprintln!("dcw: error: neither origin/{} nor local {} exists in {}.", cfg.base, cfg.base, cfg.repo.display());
        eprintln!("       fetch the base branch, or set DEV_CAPTAIN_BASE=<branch>.");
        return 2;
    };
    if !added {
        eprintln!("dcw: error: could not add worktree for {branch}");
        return 1;
    }

    // copy .env from main repo into the worktree
    for name in ["hmx/.env".to_string(), ".env".to_string()] {
        let src = cfg.repo.join(&name);
        let dst = cfg.dir_for(&branch).join(&name);
        if src.is_file() && !dst.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&src, &dst).is_ok() {
                println!("  env:     copied {} → worktree", name);
            }
        }
    }

    db::copy_private_paths(cfg, &branch);
    db::db_clone(cfg, &branch);
    db::set_worktree_env(cfg, &branch);

    let port = match port_alloc(cfg, &branch) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    bring_up(cfg, &branch, &wt, port, false)
}

/// ── start ────────────────────────────────────────────────────────
/// Start server + worker for an EXISTING worktree (or the base branch, which runs
/// in $REPO itself). Never creates a worktree — that is `dcw new`'s job.
pub fn start(cfg: &Config, branch: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let branch = match branch {
        Some(b) => b,
        None => return crate::picker::start_pick(cfg),
    };
    if !cfg.repo.join(".git").exists() {
        eprintln!("error: {} is not a git repo — clone first", cfg.repo.display());
        return 2;
    }

    if branch == cfg.base {
        // $REPO serves whatever branch it has checked out; say so when that differs.
        if let Some(head) = git::current_branch(&cfg.repo) {
            if head != cfg.base {
                eprintln!("warning: {} has '{head}' checked out, not {} — this server runs {head}.", cfg.repo.display(), cfg.base);
                eprintln!("         run 'git -C {} switch {}' to serve {}.", cfg.repo.display(), cfg.base, cfg.base);
            }
        }
        return bring_up(cfg, &branch, &cfg.repo, cfg.port_base, true);
    }

    let wt = cfg.dir_for(&branch);
    if !wt.exists() {
        eprintln!("error: no worktree for {branch} — create it first: dcw new {branch}");
        return 2;
    }
    let port = match port_alloc(cfg, &branch) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    bring_up(cfg, &branch, &wt, port, false)
}

/// Allocate/announce the port and start server + worker for an existing tree.
/// Shared by `new` (after creation) and `start`.
fn bring_up(cfg: &Config, branch: &str, wt: &Path, port: u16, base: bool) -> i32 {
    if let Some(pid) = pid_for_port(cfg, port) {
        println!("✓ {branch} already running (pid {pid}) on port {port}");
        println!("  preview: {}", cfg.url_for(port, branch));
        let _ = celery::celery_start(cfg, branch, false);
        return 0;
    }
    let log = server_log(cfg, branch);
    match start_server(cfg, wt, port, branch) {
        Some(pid) => {
            if base {
                println!("✓ captain started: {branch} (base)");
                println!("  dir:     {}", wt.display());
            } else {
                println!("✓ captain started: {branch}");
                println!("  worktree: {}", wt.display());
            }
            println!("  port:    {port}  (pid {pid})");
            println!("  preview: {}", cfg.url_for(port, branch));
            println!("  log:     {}", log.display());
        }
        None => {
            println!("⚠ port {port} reserved; server NOT started (spawn manually when ready)");
        }
    }
    let _ = celery::celery_start(cfg, branch, false);
    0
}

/// ── stop ─────────────────────────────────────────────────────────
pub fn stop(cfg: &Config, branch: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let branch = match branch {
        Some(b) => b,
        None => return crate::picker::stop_pick(cfg),
    };
    let port = match port_of(cfg, &branch) {
        Some(p) => p,
        None if branch == cfg.base => cfg.port_base,
        None => {
            println!("no captain running for {branch}");
            celery::celery_stop(cfg, &branch);
            return 0;
        }
    };
    kill_port(cfg, port);
    celery::celery_stop(cfg, &branch);
    println!("✓ stopped {branch} (port {port}) — worktree/branch left intact");
    0
}

/// ── abort ────────────────────────────────────────────────────────
pub fn abort(cfg: &Config, branch: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let branch = match branch {
        Some(b) => b,
        None => return crate::picker::abort_pick(cfg),
    };
    if branch == cfg.base {
        eprintln!("error: cannot abort '{branch}' — that would destroy {}. Use 'dcw stop {branch}' to just stop the server.", cfg.repo.display());
        return 2;
    }
    let wt = cfg.dir_for(&branch);
    let port = port_of(cfg, &branch);
    let port_str = port.map(|p| p.to_string()).unwrap_or_else(|| "-".into());

    print_destroy_summary(cfg, &[(branch.clone(), port_str)]);
    if !confirm_cached() {
        println!("aborted");
        return 1;
    }
    println!();
    abort_one(cfg, &branch, &wt, port);
    0
}

/// Print the "this will be destroyed" summary (shared by single- and multi-target
/// abort so both read the same way). Takes (branch, port-as-string) pairs.
pub fn print_destroy_summary(cfg: &Config, targets: &[(String, String)]) {
    println!("Will destroy:");
    for (branch, port) in targets {
        println!("  {branch}  →  {}  (port {port})", cfg.dir_for(branch).display());
    }
    println!("NO merge.");
}

fn confirm_cached() -> bool {
    use std::io::Write;
    print!("Proceed? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        println!(); // keep the next line clean on EOF
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Destroy worktree + port + worker + DB for one branch (every step non-fatal, so a
/// multi-target abort keeps going if one target fails).
pub fn abort_one(cfg: &Config, branch: &str, wt: &Path, port: Option<u16>) {
    if let Some(port) = port {
        kill_port(cfg, port);
        port_free(cfg, &port.to_string());
    }
    celery::celery_stop(cfg, branch);
    if wt.exists() && !git::worktree_remove(&cfg.repo, wt) {
        eprintln!("dcw: warning: could not remove worktree {}", wt.display());
    }
    git::branch_delete(&cfg.repo, branch);
    db::db_drop(cfg, branch);
    println!("✓ aborted {branch}");
}

/// ── list ─────────────────────────────────────────────────────────
/// One source of truth: git worktrees, annotated with each branch's runtime state
/// (port, pid, running/dead, celery) from the registry. Rendered as a box table
/// like dcw's `status`.
pub fn list(cfg: &Config) {
    if !cfg.repo.join(".git").exists() {
        eprintln!("error: {} is not a git repo", cfg.repo.display());
        return;
    }
    let reg = registry::load(&cfg.reg_file);
    // branch -> (port, pid, status, celery)
    let runtime: std::collections::HashMap<String, (String, i32, String, String)> = reg
        .iter()
        .filter_map(|(port, info)| {
            if info.branch.is_empty() {
                return None; // broken entry: must not match a detached worktree's ""
            }
            let Ok(port_n) = port.parse::<u16>() else { return None };
            let alive = info.pid > 0 && process::pid_alive(info.pid);
            let listening = process::port_bound(port_n);
            let status = if alive || listening { "up" } else { "dead" };
            let cel = celery_status(cfg, &info.branch);
            Some((info.branch.clone(), (port.clone(), info.pid, status.to_string(), cel)))
        })
        .collect();

    let worktrees = git::list_worktrees(&cfg.repo);
    if worktrees.is_empty() {
        println!("(no worktrees)");
        return;
    }

    let mut entries: Vec<((u8, u16), Vec<String>)> = Vec::new();
    for wt in &worktrees {
        // Runtime state is keyed by BRANCH in the registry, but the column shows
        // the WORKTREE's own name (its directory), not the branch ref.
        let branch = wt.branch.clone().unwrap_or_default();
        let name = worktree_name(cfg, wt);
        let (port, pid, status, cel) = runtime
            .get(&branch)
            .cloned()
            .unwrap_or_else(|| ("-".into(), 0, "dead".into(), "-".into()));
        let port_n = port.parse::<u16>().ok();
        let pid_str = if pid > 0 { pid.to_string() } else { "-".into() };
        // Preview URL like `dcw status`; only meaningful when the branch has a port.
        let url = match port_n {
            Some(p) => cfg.url_for(p, &branch),
            None => "-".into(),
        };
        // Sort: up first, then by port ascending ("-" ports fall to the end).
        let key = (if status == "up" { 0 } else { 1 }, port_n.unwrap_or(u16::MAX));
        entries.push((key, vec![name, pid_str, status, cel, url]));
    }
    entries.sort_by_key(|(k, _)| *k);
    let rows: Vec<Vec<String>> = entries.into_iter().map(|(_, r)| r).collect();
    render_box(&["WORKTREE", "PID", "STATUS", "CELERY", "URL"], &rows);
}

/// A worktree's own name: its directory under $WT_DIR (e.g. `PRD_INV_00296`, or
/// `feat/x` for a slashed branch), `(repo)` for the main checkout, or the full
/// directory path when the worktree lives outside $WT_DIR.
fn worktree_name(cfg: &Config, wt: &git::Worktree) -> String {
    if wt.dir == cfg.repo {
        return "(repo)".to_string();
    }
    if let Ok(rel) = wt.dir.strip_prefix(&cfg.wt_dir) {
        if !rel.as_os_str().is_empty() {
            return rel.to_string_lossy().into_owned();
        }
    }
    wt.dir.to_string_lossy().into_owned()
}

/// Render a Unicode box-drawing table (ASCII fallback, like dcw's `box()`).
fn render_box(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    // Unicode when stdout can encode it (mirrors dcw's locale check).
    let unicode = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .map(|l| !l.contains("C") && !l.contains("POSIX"))
        .unwrap_or(true);

    let (tl, tm, tr, ml, mm, mr, bl, bm, br, h, v): (char, char, char, char, char, char, char, char, char, &str, &str) =
        if unicode {
            ('┌', '┬', '┐', '├', '┼', '┤', '└', '┴', '┘', "─", "│")
        } else {
            ('+', '+', '+', '+', '+', '+', '+', '+', '+', "-", "|")
        };

    let rule = |l: char, m: char, r: char| -> String {
        let mut s = l.to_string();
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                s.push(m);
            }
            s.push_str(&h.repeat(w + 2));
        }
        s.push(r);
        s
    };

    let line = |cells: &[String]| -> String {
        let mut s = v.to_string();
        for (i, w) in widths.iter().enumerate() {
            let cell = cells.get(i).map(|c| c.as_str()).unwrap_or("");
            s.push_str(&format!(" {cell:<width$} ", width = w));
            s.push_str(v);
        }
        s
    };

    println!("{}", rule(tl, tm, tr));
    println!("{}", line(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>()));
    println!("{}", rule(ml, mm, mr));
    for row in rows {
        println!("{}", line(row));
    }
    println!("{}", rule(bl, bm, br));
}

/// ── jump ─────────────────────────────────────────────────────────
/// Print the path of the worktree the user picks with the arrow keys. The menu is
/// drawn on /dev/tty and ONLY the path goes to stdout, so the installed `dcw` shell
/// function can do `cd "$(dcw jump)"` without capturing the menu too.
pub fn jump(cfg: &Config) -> i32 {
    crate::picker::jump_pick(cfg)
}

/// ── logs ─────────────────────────────────────────────────────────
pub fn logs(cfg: &Config, branch: Option<String>, celery_flag: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let branch = match branch {
        Some(b) => b,
        None => return crate::picker::logs_pick(cfg),
    };
    let log = if celery_flag.as_deref() == Some("celery") {
        celery::logfile(cfg, &branch)
    } else {
        server_log(cfg, &branch)
    };
    if !log.exists() {
        eprintln!("no log at {}", log.display());
        return 2;
    }
    tail_follow(&log);
    0
}

fn celery_status(cfg: &Config, branch: &str) -> String {
    let pf = cfg.celery_state_dir.join(format!("{}.pid", cfg.slug(branch)));
    let Ok(text) = std::fs::read_to_string(&pf) else {
        return "-".into();
    };
    let Some(pid) = text.lines().next().and_then(|l| l.trim().parse::<i32>().ok()) else {
        return "-".into();
    };
    if !process::pid_alive(pid) {
        return "dead".into();
    }
    let log = celery::logfile(cfg, branch);
    if std::fs::read_to_string(&log).map(|s| s.contains(" ready.")).unwrap_or(false) {
        "ready".into()
    } else {
        "starting".into()
    }
}

/// `tail -f` in-process on one file.
fn tail_follow(path: &Path) {
    use std::io::{BufRead, Seek, SeekFrom};
    let file = std::fs::File::open(path).expect("log opened");
    let mut reader = std::io::BufReader::new(file);
    let mut pos = reader.seek(SeekFrom::End(0)).unwrap_or(0);
    loop {
        let _ = reader.seek(SeekFrom::Start(pos));
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => print!("{line}"),
                Err(_) => return,
            }
        }
        pos = reader.stream_position().unwrap_or(pos);
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}