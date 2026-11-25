// commands.rs — the subcommand handlers and the port/server lifecycle.

use crate::config::Config;
use crate::progress::TaskHandle;
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

/// A dcw target resolved from a user-supplied name OR branch. The identity is the
/// worktree's DIRECTORY name (stable across `git switch`), not its checked-out
/// branch — a worktree can be switched to another branch at any time, while its
/// name keeps pointing at the same directory, port, logs, and worker.
pub struct Target {
    pub name: String,
    pub dir: PathBuf,
    pub branch: Option<String>,
}

/// Canonical target for the main checkout ($REPO). The base branch's server always
/// lives in $REPO itself on port_base; its stable identity is cfg.base.
fn base_target(cfg: &Config) -> Target {
    Target {
        name: cfg.base.clone(),
        dir: cfg.repo.clone(),
        branch: git::current_branch(&cfg.repo),
    }
}

/// True when `arg` names $REPO itself by path (`.`, or a path that resolves to the
/// repo root). Lets `dcw start .` / `dcw start /path/to/repo` address the main
/// checkout (repo utama, bukan worktree) without knowing the base branch name.
fn is_repo_path(cfg: &Config, arg: &str) -> bool {
    if arg == "." {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd == cfg.repo || same_dir(&cwd, &cfg.repo);
        }
        return false;
    }
    let p = Path::new(arg);
    let path_shaped = p.is_absolute() || arg.contains('/') || arg == "..";
    if path_shaped {
        return p == cfg.repo || same_dir(p, &cfg.repo);
    }
    false
}

/// True when `arg` is a filesystem path rather than a worktree/branch name. Such an
/// argument must NOT fall through to the `dir_for()` guess: `dir_for(".")` answers
/// `$WT_DIR/.` and `dir_for("")` answers `$WT_DIR` itself, both of which exist
/// whenever `.worktrees/` does — so `dcw start .` or `dcw start ""` run from OUTSIDE
/// the repo would otherwise invent a bogus target and reserve a port.
fn is_path_arg(arg: &str) -> bool {
    Path::new(arg).is_absolute() || arg == "." || arg == ".." || arg.is_empty()
}

/// Locate a target by its worktree directory name, by the branch checked out in it,
/// or (for the main checkout, which is not a worktree under .worktrees/) by the base
/// branch name, the "(repo)" label shown by `dcw list`, or a path resolving to $REPO.
///
/// The returned name is dcw's stable identity: it keeps the port, the log file, and
/// the celery pidfile when the user `git switch`es the worktree to a different branch.
/// The main checkout canonicalizes to cfg.base so its port/log/registry stay
/// consistent regardless of how it was named.
pub fn find_target(cfg: &Config, arg: &str) -> Option<Target> {
    if arg == cfg.base || arg == "(repo)" || is_repo_path(cfg, arg) {
        return Some(base_target(cfg));
    }
    for wt in git::list_worktrees(&cfg.repo) {
        let name = worktree_name(cfg, &wt);
        if name != arg && wt.branch.as_deref() != Some(arg) {
            continue;
        }
        // The main checkout may appear in git's worktree list: canonicalize it.
        if wt.dir == cfg.repo || same_dir(&wt.dir, &cfg.repo) {
            return Some(base_target(cfg));
        }
        return Some(Target {
            name,
            dir: wt.dir.clone(),
            branch: wt.branch.clone(),
        });
    }
    // A worktree git does not list (detached, or not yet registered): fall back to
    // the path dcw would have created for it. Never guess from a path-shaped arg —
    // `dir_for(".")` exists whenever .worktrees/ does, and would invent a bogus target.
    if !is_path_arg(arg) {
        let dir = cfg.dir_for(arg);
        if dir.is_dir() {
            if dir == cfg.repo || same_dir(&dir, &cfg.repo) {
                return Some(base_target(cfg));
            }
            let branch = git::current_branch(&dir);
            return Some(Target {
                name: arg.to_string(),
                dir,
                branch,
            });
        }
    }
    None
}

/// Port recorded for a target: by its worktree name first (the key dcw allocated the
/// port under), then by its currently checked-out branch (older registry entries).
pub fn port_for(cfg: &Config, t: &Target) -> Option<u16> {
    // The base checkout always lives on PORT_BASE even when nothing is registered
    // yet, so `dcw stop <base>` works on a never-started / fully-stopped captain.
    if t.dir == cfg.repo {
        return Some(cfg.port_base);
    }
    port_of(cfg, &t.name).or_else(|| t.branch.as_deref().and_then(|b| port_of(cfg, b)))
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
    let _lock = Lock::acquire(cfg.lock_dir.clone());
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

/// Is this registry entry a leftover whose worktree no longer exists? `port_free` only
/// runs from `dcw abort`, so a worktree removed any other way keeps its port reserved
/// forever and every later `new` starts one number higher.
fn is_orphan(info: &registry::PortInfo, base: &str, live: &std::collections::HashSet<String>) -> bool {
    info.branch != base && !live.contains(&info.branch) && !process::pid_alive(info.pid)
}

/// Allocate the first free port in [PORT_BASE+1, PORT_MAX-1] for a branch, claiming it.
fn port_alloc(cfg: &Config, branch: &str) -> Result<u16, String> {
    let _lock = Lock::acquire(cfg.lock_dir.clone());
    if let Some(p) = port_of(cfg, branch) {
        return Ok(p);
    }
    let mut reg = registry::load(&cfg.reg_file);
    // Reclaim ports of worktrees that are gone from `git worktree list`.
    let live: std::collections::HashSet<String> = git::list_worktrees(&cfg.repo)
        .iter()
        .map(|wt| worktree_name(cfg, wt))
        .collect();
    let before = reg.len();
    reg.retain(|_, info| !is_orphan(info, &cfg.base, &live));
    if reg.len() != before {
        registry::save(&cfg.reg_file, &reg);
    }
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
/// pid=0 tombstone so its port stays reserved). `grace_secs` is the graceful window
/// for the recorded server: `dcw stop` allows 2s, `abort` (tree about to be deleted)
/// passes 0.
fn kill_port(cfg: &Config, port: u16, grace_secs: u64) {
    // The recorded pid is the group leader (spawn_detached setsid's it), so this
    // reaches the server's children too.
    let rec = registry::load(&cfg.reg_file)
        .get(&port.to_string())
        .and_then(|i| (i.pid > 0).then_some(i.pid));
    if let Some(pid) = rec {
        process::stop_group(pid, grace_secs);
    }
    // Anything still listening that dcw did not spawn itself.
    for pid in lsof_listeners(port) {
        process::signal(pid, nix::sys::signal::Signal::SIGKILL);
    }
    // clear pid (keep the entry for the base branch; drop the entry otherwise).
    let _lock = Lock::acquire(cfg.lock_dir.clone());
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

pub fn server_log(cfg: &Config, key: &str) -> PathBuf {
    if key == cfg.base {
        cfg.logs_dir.join("main.log")
    } else {
        cfg.logs_dir.join(format!("{}.log", cfg.slug(key)))
    }
}

fn ensure_dirs(cfg: &Config) {
    let _ = std::fs::create_dir_all(&cfg.logs_dir);
    let _ = std::fs::create_dir_all(&cfg.celery_state_dir);
}

/// ── new ──────────────────────────────────────────────────────────
/// Create NEW worktree(s) and bring them up. Errors if a worktree already exists —
/// running an existing one is `dcw start`'s job.
///
/// Several names run at once, at most `MAX_PARALLEL_NEW` at a time. The cap is not
/// politeness: the checkout and `createdb -T` are disk-bound (measured at 94% disk
/// utilisation), so extra concurrency buys little while costing ~1.3 GB of RAM per
/// running worktree (server + worker) and a fifth of the free disk.
pub fn create(cfg: &Config, branches: Vec<String>) -> i32 {
    ensure_dirs(cfg);
    if branches.is_empty() {
        eprintln!("usage: dcw new <branch> [branch...]   (create worktrees; 'dcw start' runs an existing one)");
        return 2;
    }
    if !cfg.repo.join(".git").exists() {
        eprintln!("error: {} is not a git repo — clone first", cfg.repo.display());
        return 2;
    }
    if branches.len() == 1 {
        return create_one(cfg, &branches[0], None);
    }

    // One fetch for the whole batch: every worktree starts from the same base ref, and
    // fetching it once per target would just repeat the same network round trip.
    let progress = crate::progress::Progress::start();
    let fetch_row = progress.task(&format!("fetch origin/{}", cfg.base), "");
    let (fetch_ok, _msg) = git::fetch_origin(&cfg.repo, &cfg.base);
    fetch_row.done();
    if !fetch_ok {
        eprintln!("dcw: warning: git fetch origin failed; using existing local refs");
    }
    // Resolve the start point once: it is the same for every target in the batch.
    let Some((start, rev)) = resolve_start(cfg, &branches[0]) else {
        progress.stop();
        return 2;
    };

    let rows: Vec<_> = branches.iter().map(|b| progress.task(b, "queued")).collect();
    // A fixed set of workers pulling from a shared index, so at most MAX_PARALLEL_NEW
    // checkouts and DB clones are ever in flight, however many names were given.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<i32> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..MAX_PARALLEL_NEW.min(branches.len()))
            .map(|_| {
                let (next, start, rev, branches, rows) = (&next, &start, &rev, &branches, &rows);
                s.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(name) = branches.get(i) else { break };
                        let row = &rows[i];
                        done.push((i, create_one(cfg, name, Some((row, start.clone(), rev.clone())))));
                    }
                    done
                })
            })
            .collect();
        let mut codes = vec![0; branches.len()];
        for w in workers {
            for (i, code) in w.join().unwrap_or_default() {
                codes[i] = code;
            }
        }
        codes
    });
    progress.stop();
    if results.iter().all(|c| *c == 0) {
        0
    } else {
        1
    }
}

/// Most `dcw new` targets built at the same time. See `create`.
const MAX_PARALLEL_NEW: usize = 3;

/// The start point for a new branch: an existing local branch (no start ref), else
/// `origin/<base>`, else local `<base>`. `None` when none of them exists.
fn resolve_start(cfg: &Config, branch: &str) -> Option<(Option<String>, String)> {
    if git::ref_exists(&cfg.repo, &format!("refs/heads/{branch}")) {
        Some((None, branch.to_string()))
    } else if git::ref_exists(&cfg.repo, &format!("refs/remotes/origin/{}", cfg.base)) {
        let r = format!("origin/{}", cfg.base);
        Some((Some(r.clone()), r))
    } else if git::ref_exists(&cfg.repo, &format!("refs/heads/{}", cfg.base)) {
        Some((Some(cfg.base.clone()), cfg.base.clone()))
    } else {
        eprintln!("dcw: error: neither origin/{} nor local {} exists in {}.", cfg.base, cfg.base, cfg.repo.display());
        eprintln!("       fetch the base branch, or set DEV_CAPTAIN_BASE=<branch>.");
        None
    }
}

/// Build one worktree and bring it up. `batch` carries the row to draw into and the
/// start point already resolved by `create`; `None` means a single-target run, which
/// does its own fetch, resolution, and progress block.
fn create_one(cfg: &Config, branch: &str, batch: Option<(&TaskHandle, Option<String>, String)>) -> i32 {
    if branch == cfg.base {
        eprintln!("error: the base branch runs in {} itself, not in a worktree.", cfg.repo.display());
        eprintln!("       use 'dcw start {branch}' to run it.");
        return 2;
    }

    let wt = cfg.dir_for(branch);
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

    // Batch: the caller already fetched and resolved the start point, and handed us the
    // row to draw into. Single: this call does both itself. Copy the batch data out so
    // the rest of the function never borrows `batch` again.
    let (batch_row, batch_start, batch_rev) = match &batch {
        Some((r, start, rev)) => (Some(*r), Some(start.clone()), Some(rev.clone())),
        None => (None, None, None),
    };
    let (single, start, rev): (Option<crate::progress::Progress>, _, _) = match batch_rev {
        Some(rev) => (None, batch_start.unwrap_or(None), rev),
        None => {
            let p = crate::progress::Progress::start();
            let fetch_row = p.task(&format!("fetch origin/{}", cfg.base), "");
            let (ok, _) = git::fetch_origin(&cfg.repo, &cfg.base);
            fetch_row.done();
            if !ok {
                eprintln!("dcw: warning: git fetch origin failed; using existing local refs");
            }
            let Some((start, rev)) = resolve_start(cfg, branch) else {
                p.stop();
                return 2;
            };
            (Some(p), start, rev)
        }
    };
    let checkout_row = single.as_ref().map(|p| p.task("checkout", ""));
    let db_row = single.as_ref().map(|p| p.task("database", "cloning template"));
    let config_row = single.as_ref().map(|p| p.task("config", ""));
    // The row the checkout bar is drawn into: the batch's own row (one row per worktree)
    // or, for a single target, its dedicated checkout row.
    let bar_row: &TaskHandle = match (batch_row, &checkout_row) {
        (Some(r), _) => r,
        (None, Some(row)) => row,
        // Unreachable: a single run always has a block, hence a row.
        (None, None) => return 1,
    };
    // The row that reports the phases after the checkout: the batch's row (which stays
    // live until the worktree is up) or, for a single target, the config row.
    let phase_row: &TaskHandle = match (batch_row, &config_row) {
        (Some(r), _) => r,
        (None, Some(row)) => row,
        (None, None) => return 1,
    };
    let total = crate::progress::checkout_file_count(&cfg.repo, &rev);
    // Stop sampling when the checkout is done, not when the count reaches 100%: the files
    // git writes need not equal the count `ls-tree` reports (a skipped or generated path
    // never lands), and a loop waiting for 100% would spin forever.
    let checkout_done = std::sync::atomic::AtomicBool::new(false);

    // The DB clone only reads $REPO and the branch name, so it runs alongside the
    // checkout (the other slow half of `dcw new`) instead of after it. The checkout row is
    // filled from "files written / files in the tree" — both measured, not estimated.
    let (db_lines, db_created, added) = std::thread::scope(|s| {
        let db = s.spawn(|| {
            let result = db::db_clone(cfg, branch);
            if let Some(r) = &db_row {
                r.done();
            }
            result
        });
        let (wt_for_sampler, row, done) = (wt.clone(), bar_row.clone(), &checkout_done);
        let sampler = s.spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                row.set_written(crate::progress::files_written(&wt_for_sampler), total);
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        });
        let added = match &start {
            Some(start) => git::worktree_add_new(&cfg.repo, &wt, branch, start),
            None => git::worktree_add_existing(&cfg.repo, &wt, branch),
        };
        checkout_done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = sampler.join();
        // Single run: the checkout row is its own row, so mark it finished here. A batch
        // row is not done yet — it still has configuring and starting to report.
        if batch_row.is_none() {
            if let Some(r) = &checkout_row {
                r.done();
            }
        }
        let (d, c) = db.join().unwrap_or_default();
        (d, c, added)
    });
    if !added {
        // Don't leave a cloned DB behind for a worktree that was never created.
        if db_created {
            db::db_drop(cfg, branch, &|m| eprintln!("  db:      {m}"));
        }
        if batch_row.is_some() {
            phase_row.finish("failed");
        }
        if let Some(p) = single {
            p.stop();
        }
        eprintln!("dcw: error: could not add worktree for {branch}");
        return 1;
    }
    // The clone's routine lines duplicate the progress row; only its warnings add
    // information the bar cannot show (a fallback to dump/restore, a skipped clone).
    for line in &db_lines {
        if line.contains("warning") || line.contains("failed") || line.contains("skipped") {
            phase_row.warn(line.trim());
        }
    }

    // .env first (the worktree's own copy), then the private files and the per-worktree
    // .env/.db.json values. All of it is reported as a few summary rows, not one per path.
    // A batch row is named after the worktree, so it needs the phase; a single run's row
    // is already labelled `config`.
    if batch_row.is_some() {
        phase_row.step("configuring");
    }
    for name in ["hmx/.env", ".env"] {
        let src = cfg.repo.join(name);
        let dst = cfg.dir_for(branch).join(name);
        if src.is_file() && !dst.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&src, &dst);
        }
    }

    // Side effects only: the paths and the .env/.db.json values are written, but the
    // checklist above is the whole output of `dcw new`, so they print nothing.
    let _ = db::copy_private_paths(cfg, branch);
    let _ = db::set_worktree_env(cfg, branch);

    let port = match port_alloc(cfg, branch) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            if batch_row.is_some() {
                phase_row.finish("failed");
            }
            if let Some(p) = single {
                p.stop();
            }
            return 1;
        }
    };
    let target = Target {
        name: branch.to_string(),
        dir: wt,
        branch: Some(branch.to_string()),
    };
    match batch_row {
        // One row per worktree: it stays live until that worktree is up.
        Some(_) => {
            phase_row.step(&format!("starting on port {port}"));
            let code = bring_up(cfg, &target, port, false, true);
            phase_row.finish(if code == 0 { "up" } else { "failed" });
            code
        }
        // Single target: the checklist ends here, and bring_up prints the preview URL.
        None => {
            phase_row.done();
            if let Some(p) = single {
                p.stop();
            }
            let code = bring_up(cfg, &target, port, false, true);
            if code == 0 {
                // Let the installed `dcw` shell function cd the CALLING shell into the new
                // worktree. A child process cannot cd its parent, so the path goes through
                // DCW_CD_FILE for the wrapper to read. Only for a single target: with
                // several, there is no one directory to cd into.
                write_cd_file(&target.dir);
            }
            code
        }
    }
}

/// Hand the caller's shell function a directory to cd into (read by the installed
/// `dcw()` wrapper via $DCW_CD_FILE). No-op when the variable is empty or writing
/// fails — a missing cd target must never fail the command.
fn write_cd_file(dir: &Path) {
    if let Ok(path) = std::env::var("DCW_CD_FILE") {
        if !path.is_empty() {
            let _ = std::fs::write(path, dir.to_string_lossy().as_bytes());
        }
    }
}


/// ── start ────────────────────────────────────────────────────────
/// Start server + worker for an EXISTING worktree (or the base branch, which runs
/// in $REPO itself). Never creates a worktree — that is `dcw new`'s job.
///
/// `name` is the WORKTREE name (its directory under .worktrees/), not the branch
/// checked out in it: a worktree keeps its name, port, logs, and worker when you
/// `git switch` inside it. A branch name is still accepted and resolved to the
/// worktree that has it checked out.
pub fn start(cfg: &Config, name: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let name = match name {
        Some(b) => b,
        None => return crate::picker::start_pick(cfg),
    };
    if !cfg.repo.join(".git").exists() {
        eprintln!("error: {} is not a git repo — clone first", cfg.repo.display());
        return 2;
    }

    let Some(target) = find_target(cfg, &name) else {
        eprintln!("error: no target named '{name}' — not a worktree, the base branch, or the repo itself");
        eprintln!("       'dcw list' shows every existing worktree (the main checkout is '(repo)')");
        return 2;
    };

    if target.dir == cfg.repo {
        // $REPO serves whatever branch it has checked out; say so when that differs.
        if let Some(head) = git::current_branch(&cfg.repo) {
            if head != cfg.base {
                eprintln!("warning: {} has '{head}' checked out, not {} — this server runs {head}.", cfg.repo.display(), cfg.base);
                eprintln!("         run 'git -C {} switch {}' to serve {}.", cfg.repo.display(), cfg.base, cfg.base);
            }
        }
        return bring_up(cfg, &target, cfg.port_base, true, false);
    }

    let port = match port_alloc(cfg, &target.name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    bring_up(cfg, &target, port, false, false)
}

/// Allocate/announce the port and start server + worker for an existing tree.
/// Shared by `new` (after creation) and `start`. The target's NAME keys the port,
/// the log file, the celery pidfile, and the registry entry.
///
/// `quiet` = the caller already showed a progress block and only wants the preview
/// URL (`dcw new`); routine status rows are dropped, but warnings and failures
/// (port reserved but not started, worker did not come up) are always printed.
fn bring_up(cfg: &Config, t: &Target, port: u16, base: bool, quiet: bool) -> i32 {
    if let Some(pid) = pid_for_port(cfg, port) {
        println!("✓ {} already running (pid {pid}) on port {port}", t.name);
        println!("  preview: {}", cfg.url_for(port, &t.name));
        let _ = celery::celery_start(cfg, &t.name, false, quiet);
        return 0;
    }
    let log = server_log(cfg, &t.name);
    match start_server(cfg, &t.dir, port, &t.name) {
        Some(pid) if quiet => {
            // The caller's checklist is the output; the URL is the one thing it
            // cannot show, since it is allocated here.
            let _ = pid;
            println!("  {}", cfg.url_for(port, &t.name));
        }
        Some(pid) => {
            if base {
                println!("✓ captain started: {} (base)", t.name);
            } else {
                println!("✓ captain started: {}", t.name);
            }
            // One label column: every label padded to the longest, so the values line up.
            let mut rows: Vec<(&str, String)> = Vec::new();
            if base {
                rows.push(("dir", t.dir.display().to_string()));
            } else {
                rows.push(("worktree", t.dir.display().to_string()));
                if let Some(branch) = &t.branch {
                    rows.push(("branch", branch.clone()));
                }
            }
            rows.push(("port", format!("{port}  (pid {pid})")));
            rows.push(("preview", cfg.url_for(port, &t.name)));
            rows.push(("log", log.display().to_string()));
            let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            for (label, value) in rows {
                println!("  {label:<width$}  {value}");
            }
        }
        None => {
            println!("⚠ port {port} reserved; server NOT started (spawn manually when ready)");
        }
    }
    let _ = celery::celery_start(cfg, &t.name, false, quiet);
    0
}

/// ── stop ─────────────────────────────────────────────────────────
pub fn stop(cfg: &Config, name: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let name = match name {
        Some(b) => b,
        None => return crate::picker::stop_pick(cfg),
    };
    let Some(target) = find_target(cfg, &name) else {
        eprintln!("error: no target named '{name}' (see 'dcw list')");
        return 2;
    };
    let port = match port_for(cfg, &target) {
        Some(p) => p,
        None if target.dir == cfg.repo => cfg.port_base,
        None => {
            println!("no captain running for {}", target.name);
            celery::celery_stop_print(cfg, &target.name, 2);
            return 0;
        }
    };
    kill_port(cfg, port, 2);
    celery::celery_stop_print(cfg, &target.name, 2);
    println!("✓ stopped {} (port {port}) — worktree/branch left intact", target.name);
    0
}

/// ── restart ──────────────────────────────────────────────────────
/// Stop then start a worktree's server + worker. The port stays sticky (stop
/// clears the registry pid but keeps the entry), and the worktree/DB are
/// untouched. Restarting a worktree that isn't running just starts it.
pub fn restart(cfg: &Config, name: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let name = match name {
        Some(b) => b,
        None => return crate::picker::restart_pick(cfg),
    };
    let code = stop(cfg, Some(name.clone()));
    if code != 0 {
        return code;
    }
    start(cfg, Some(name))
}

/// ── abort ────────────────────────────────────────────────────────
pub fn abort(cfg: &Config, name: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let name = match name {
        Some(b) => b,
        None => return crate::picker::abort_pick(cfg),
    };
    let Some(target) = find_target(cfg, &name) else {
        eprintln!("error: no target named '{name}' (see 'dcw list')");
        return 2;
    };
    if target.dir == cfg.repo {
        eprintln!("error: cannot abort '{}' — that would destroy {}. Use 'dcw stop {}' to just stop the server.", target.name, cfg.repo.display(), target.name);
        return 2;
    }
    let port = port_for(cfg, &target);
    let port_str = port.map(|p| p.to_string()).unwrap_or_else(|| "-".into());

    print_destroy_summary(cfg, &[(target.name.clone(), port_str)]);
    if !confirm_cached() {
        println!("aborted");
        return 1;
    }
    println!();
    let progress = crate::progress::Progress::start();
    let row = progress.task(&target.name, "");
    abort_one(cfg, &target.name, &target.dir, port, &row);
    progress.stop();
    0
}

/// Print the "this will be destroyed" summary (shared by single- and multi-target
/// abort so both read the same way). Takes (worktree-name, port-as-string) pairs.
pub fn print_destroy_summary(cfg: &Config, targets: &[(String, String)]) {
    println!("Will destroy:");
    for (name, port) in targets {
        let dir = match find_target(cfg, name) {
            Some(t) => t.dir,
            None => cfg.dir_for(name),
        };
        let branch = git::current_branch(&dir);
        println!("  {name}  →  {}  (port {port})", dir.display());
        if let Some(b) = branch {
            if &b != name {
                println!("     worktree is on branch '{b}' (that branch ref is kept)");
            }
        }
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

/// Destroy worktree + port + worker + DB for one worktree (every step non-fatal, so a
/// multi-target abort keeps going if one target fails).
///
/// `name` is the WORKTREE name (dcw's identity, and the DB name). The branch ref
/// that a `git switch` put in the worktree is NOT deleted — that is the user's work,
/// not something dcw created — only the ref matching the worktree name is.
///
/// `row` is this target's progress row; every phase reports through it, so several
/// aborts running at once never interleave their output.
pub fn abort_one(cfg: &Config, name: &str, wt: &Path, port: Option<u16>, row: &TaskHandle) {
    if let Some(port) = port {
        row.step("stopping server");
        kill_port(cfg, port, 0);
        port_free(cfg, &port.to_string());
    }
    // The worktree (and its DB) is about to go, so the worker's queues and in-flight
    // tasks have nowhere to land: skip the graceful drain entirely.
    celery::celery_stop(cfg, name, 0, &|m| row.step(m));
    // Capture what is checked out before the worktree goes away.
    let checked_out = git::current_branch(wt);
    row.step("removing worktree");
    if wt.exists() && !git::worktree_remove(&cfg.repo, wt) {
        row.step(&format!("warning: could not remove {}", wt.display()));
    }
    git::branch_delete(&cfg.repo, name);
    if let Some(branch) = checked_out {
        if branch != name {
            row.step(&format!("kept branch '{branch}'"));
        }
    }
    db::db_drop(cfg, name, &|m| row.step(m));
    row.finish("aborted");
}

/// ── list ─────────────────────────────────────────────────────────
/// One source of truth: git worktrees, annotated with each worktree's runtime state
/// (port, pid, running/dead, celery) from the registry. Rendered as a box table
/// like dcw's `status`. The WORKTREE column is the identity dcw uses everywhere
/// (directory name → port, logs, worker); BRANCH shows what is checked out in it
/// right now, which can differ after a `git switch` inside the worktree.
pub fn list(cfg: &Config) {
    if !cfg.repo.join(".git").exists() {
        eprintln!("error: {} is not a git repo", cfg.repo.display());
        return;
    }
    let reg = registry::load(&cfg.reg_file);
    // key -> (port, pid, status, celery). Registry keys are worktree names (they
    // were the branch names at creation time); a `git switch` inside the worktree
    // changes the checked-out branch but never the name, so the name is the key
    // that keeps matching. Fall back to the checked-out branch for entries written
    // before this rule.
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
        let name = worktree_name(cfg, wt);
        // Look the runtime state up by WORKTREE NAME first, then by whatever branch
        // is checked out in it.
        let branch = wt.branch.clone().unwrap_or_default();
        let (port, pid, status, cel) = runtime
            .get(&name)
            .or_else(|| runtime.get(&branch))
            .cloned()
            .unwrap_or_else(|| ("-".into(), 0, "dead".into(), "-".into()));
        let port_n = port.parse::<u16>().ok();
        let pid_str = if pid > 0 { pid.to_string() } else { "-".into() };
        // Preview URL like `dcw status`; a port's URL is fixed by its number.
        let url = match port_n {
            Some(p) => cfg.url_for(p, &name),
            None => "-".into(),
        };
        // Always show what is checked out: after a `git switch` it differs from the
        // name, and hiding it when it matches makes the column look broken.
        let branch_str = if branch.is_empty() {
            "(detached)".into()
        } else {
            branch
        };
        // Sort: up first, then by port ascending ("-" ports fall to the end).
        let key = (if status == "up" { 0 } else { 1 }, port_n.unwrap_or(u16::MAX));
        entries.push((key, vec![name, branch_str, pid_str, status, cel, url]));
    }
    entries.sort_by_key(|(k, _)| *k);
    let rows: Vec<Vec<String>> = entries.into_iter().map(|(_, r)| r).collect();
    render_box(&["WORKTREE", "BRANCH", "PID", "STATUS", "CELERY", "URL"], &rows);
}

/// A worktree's own name: its directory under $WT_DIR (e.g. `PRD_INV_00296`, or
/// `feat/x` for a slashed branch), `(repo)` for the main checkout, or the full
/// directory path when the worktree lives outside $WT_DIR.
pub fn worktree_name(cfg: &Config, wt: &git::Worktree) -> String {
    // `git worktree list` prints realpaths, so a repo reached through a symlink (macOS
    // /tmp -> /private/tmp, /var -> /private/var, or a symlinked home) would never
    // match the configured prefix. Compare canonicalized paths as a second chance.
    if wt.dir == cfg.repo || same_dir(&wt.dir, &cfg.repo) {
        return "(repo)".to_string();
    }
    if let Ok(rel) = wt.dir.strip_prefix(&cfg.wt_dir) {
        if !rel.as_os_str().is_empty() {
            return rel.to_string_lossy().into_owned();
        }
    }
    if let Ok(rel) = relative_through_symlinks(&wt.dir, &cfg.wt_dir) {
        if !rel.as_os_str().is_empty() {
            return rel.to_string_lossy().into_owned();
        }
    }
    wt.dir.to_string_lossy().into_owned()
}

/// True when both paths point at the same directory once symlinks are resolved.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `wt_dir`-relative path, resolving symlinks on both sides (macOS /tmp is a symlink).
fn relative_through_symlinks(dir: &Path, base: &Path) -> std::io::Result<PathBuf> {
    let dir = dir.canonicalize()?;
    let base = base.canonicalize()?;
    dir.strip_prefix(&base)
        .map(|p| p.to_path_buf())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::NotFound))
}

/// Render a Unicode box-drawing table (ASCII fallback, like dcw's `box()`).
pub fn render_box(headers: &[&str], rows: &[Vec<String>]) {
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
/// `arg` is a WORKTREE name (its directory), like every other per-target command;
/// a branch name is accepted and resolved to the worktree that has it checked out.
pub fn logs(cfg: &Config, arg: Option<String>, celery_flag: Option<String>) -> i32 {
    ensure_dirs(cfg);
    let name = match arg {
        Some(b) => b,
        None => return crate::picker::logs_pick(cfg),
    };
    // `(repo)` / base resolve to the base checkout, whose identity is the base name.
    let name = match find_target(cfg, &name) {
        Some(t) => t.name,
        None => {
            eprintln!("error: no worktree named '{name}' (see 'dcw list')");
            return 2;
        }
    };
    let log = if celery_flag.as_deref() == Some("celery") {
        celery::logfile(cfg, &name)
    } else {
        server_log(cfg, &name)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(branch: &str, pid: i32) -> registry::PortInfo {
        registry::PortInfo { branch: branch.into(), pid, started_at: None }
    }

    #[test]
    fn orphan_is_a_dead_worktree_that_is_not_base() {
        let live: std::collections::HashSet<String> = ["PRD_INV_1".to_string()].into();
        // dead worktree, no live pid: reclaimed
        assert!(is_orphan(&info("fix", 0), "Inventory", &live));
        // worktree still exists (even if its server is down): kept
        assert!(!is_orphan(&info("PRD_INV_1", 0), "Inventory", &live));
        // server still alive: kept even if git does not list it
        assert!(!is_orphan(&info("stray", std::process::id() as i32), "Inventory", &live));
        // the base branch lives in $REPO, never in the worktree list: kept
        assert!(!is_orphan(&info("Inventory", 0), "Inventory", &live));
    }
}
