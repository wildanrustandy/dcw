// celery.rs — per-branch Celery worker lifecycle (start/stop/readiness/namespace).

use crate::config::Config;
use crate::process;
use std::path::PathBuf;

fn pidfile(cfg: &Config, branch: &str) -> PathBuf {
    cfg.celery_state_dir.join(format!("{}.pid", cfg.slug(branch)))
}

pub fn logfile(cfg: &Config, branch: &str) -> PathBuf {
    cfg.logs_dir.join(format!("{}-celery.log", cfg.slug(branch)))
}

/// The django root (dir containing manage.py) for a branch.
fn django_root(cfg: &Config, branch: &str) -> Option<PathBuf> {
    let tree = cfg.dir_for(branch);
    for cand in [".", "hmx", "src", "backend", "app"] {
        let candidate = tree.join(cand).join("manage.py");
        if candidate.is_file() {
            return Some(tree.join(cand));
        }
    }
    None
}

/// Live worker pid read from the pidfile's first line; empty if dead/zombie.
fn celery_pid(cfg: &Config, branch: &str) -> Option<i32> {
    let pf = pidfile(cfg, branch);
    let text = std::fs::read_to_string(&pf).ok()?;
    let pid: i32 = text.lines().next()?.trim().parse().ok()?;
    process::pid_alive(pid).then_some(pid)
}

/// Namespace a branch's .env resolves to (empty if unknown).
fn celery_namespace(cfg: &Config, branch: &str) -> String {
    crate::db::env_value(&cfg.dir_for(branch).join("hmx/.env"), "HMX_INSTANCE_NAMESPACE")
}

/// Has the worker printed "<hostname> ready." in its log? Checks dcw's log then
/// dcw's, so a worker dcw started also reads as ready.
fn celery_ready(cfg: &Config, branch: &str) -> bool {
    std::fs::read_to_string(logfile(cfg, branch))
        .map(|s| s.contains(" ready."))
        .unwrap_or(false)
}

/// Start the worker. `wait` = block until ready (explicit `dcw celery start`), else
/// only confirm it survived startup (the `dcw create` fast path). Returns Ok(pid)
/// when a worker is up, Err(()) when a requested worker did not come up.
pub fn celery_start(cfg: &Config, branch: &str, wait: bool) -> Result<i32, ()> {
    if !cfg.celery_enabled {
        println!("  celery:  skipped (DCW_CELERY=0)");
        return Err(());
    }
    if let Some(pid) = celery_pid(cfg, branch) {
        println!("  celery:  already running (pid {pid})");
        return Ok(pid);
    }
    let Some(root) = django_root(cfg, branch) else {
        eprintln!("  celery:  django root not found; worker NOT started");
        return Err(());
    };
    let Some(celery_bin) = process::venv_bin(&cfg.venv, "celery") else {
        eprintln!("  celery:  celery binary not found; worker NOT started");
        return Err(());
    };

    let log = logfile(cfg, branch);
    let hostname = format!("dcw-{}@%h", cfg.slug(branch));
    let args = vec![
        "-A".to_string(), "hmx".to_string(),
        "worker".to_string(),
        format!("--loglevel={}", cfg.celery_loglevel),
        format!("--concurrency={}", cfg.celery_concurrency),
        format!("--hostname={hostname}"),
        "-Q".to_string(), cfg.celery_queues.clone(),
    ];

    let pid = match process::spawn_detached(&root, &log, celery_bin.to_str().unwrap_or("celery"), &args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  celery:  failed to spawn worker: {e}");
            return Err(());
        }
    };

    // pidfile: pid + branch (2 lines), so `status` can name the branch behind the slug.
    let pf = pidfile(cfg, branch);
    let _ = std::fs::create_dir_all(&cfg.celery_state_dir);
    let _ = std::fs::write(&pf, format!("{pid}\n{branch}\n"));

    // wait up to ~10s for the process to appear
    let mut got = None;
    for _ in 0..20 {
        got = celery_pid(cfg, branch);
        if got.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let Some(pid) = got else {
        eprintln!("  celery:  worker did NOT start (server is unaffected); last log lines:");
        tail(log, 15);
        let _ = std::fs::remove_file(&pf);
        return Err(());
    };

    // readiness: fast path ~2s, explicit path ~60s (HMX loads modules ~15-30s)
    let budget = if wait { 120 } else { 4 };
    let mut ready = false;
    for _ in 0..budget {
        if celery_pid(cfg, branch).is_none() {
            // died
            eprintln!("  celery:  worker died during startup (server is unaffected); last log lines:");
            tail(log, 15);
            let _ = std::fs::remove_file(&pf);
            return Err(());
        }
        if celery_ready(cfg, branch) {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    let ns = celery_namespace(cfg, branch);
    if ready {
        println!("  celery:  worker up (pid {pid}, ns {ns})");
        return Ok(pid);
    }
    if wait {
        eprintln!("  celery:  worker alive but never became ready (~60s); see {}", log.display());
        return Err(());
    }
    println!("  celery:  worker starting (pid {pid}, ns {ns})");
    println!("           HMX module load takes ~15-30s; watch: dcw logs {branch} celery");
    Ok(pid)
}

/// Stop the branch's worker (keeps branch/worktree). dcw stop sequence.
pub fn celery_stop(cfg: &Config, branch: &str) {
    let Some(pid) = celery_pid(cfg, branch) else {
        let _ = std::fs::remove_file(pidfile(cfg, branch));
        println!("  celery:  not running");
        return;
    };
    process::terminate_and_wait(pid, 15);
    let _ = std::fs::remove_file(pidfile(cfg, branch));
    println!("  celery:  stopped (pid {pid})");
}

/// Print the tail of a log file (best-effort).
fn tail(path: PathBuf, n: usize) {
    if let Ok(text) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(n);
        for l in &lines[start..] {
            eprintln!("    {l}");
        }
    }
}