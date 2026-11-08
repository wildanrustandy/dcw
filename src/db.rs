// db.rs — per-worktree Postgres DB and .env/db.json rewriting.
//
// db_clone: clone the main DB (hmxmain) into a per-branch DB via `createdb -T`,
// falling back to dump/restore when the template clone fails (source DB in use).
// Same credential flow as dcw: read DATABASE_* from the main repo's hmx/.env.

use crate::config::Config;
use std::path::Path;
use std::process::Command;

/// Read KEY=VALUE from a .env file (empty string if missing/unreadable).
pub fn env_value(path: &Path, key: &str) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                return value.trim().trim_matches('"').to_string();
            }
        }
    }
    String::new()
}

struct Conn {
    user: String,
    password: String,
    host: String,
    port: String,
}

/// Credentials from the main repo's hmx/.env, with dcw's defaults.
fn conn_vars(cfg: &Config) -> Conn {
    let env_file = cfg.repo.join("hmx/.env");
    Conn {
        user: env_value(&env_file, "DATABASE_USER"),
        password: env_value(&env_file, "DATABASE_PASSWORD"),
        host: {
            let h = env_value(&env_file, "DATABASE_HOST");
            if h.is_empty() { "127.0.0.1".into() } else { h }
        },
        port: {
            let p = env_value(&env_file, "DATABASE_PORT");
            if p.is_empty() { "5432".into() } else { p }
        },
    }
}

/// The default DATABASE name, from db.json's default.NAME, else .env DATABASE_NAME.
fn default_db_name(cfg: &Config) -> String {
    let dbjson = cfg.repo.join("hmx/hmx/db.json");
    if let Ok(text) = std::fs::read_to_string(&dbjson) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(name) = v
                .get("default")
                .and_then(|d| d.get("NAME"))
                .and_then(|n| n.as_str())
            {
                return name.to_string();
            }
        }
    }
    env_value(&cfg.repo.join("hmx/.env"), "DATABASE_NAME")
}

/// Run psql with the given args under PGPASSWORD; returns success.
#[allow(dead_code)]
fn psql(conn: &Conn, args: &[&str]) -> bool {
    Command::new("psql")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user])
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn db_exists(conn: &Conn, name: &str) -> bool {
    let out = Command::new("psql")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user, "-d", "postgres", "-Atqc"])
        .arg(format!("SELECT 1 FROM pg_database WHERE datname = '{name}'"))
        .output();
    match out {
        Ok(o) => o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "1",
        Err(_) => false,
    }
}

/// Clone the main DB into the branch's DB. Never fatal to the caller's flow if tools
/// are missing (dcw skips DB clone when psql is absent). Returns a message.
pub fn db_clone(cfg: &Config, branch: &str) {
    let dst = cfg.db_name(branch);
    let Some(conn) = (crate::process::which("psql").is_some()).then(|| conn_vars(cfg)) else {
        eprintln!("  db:      psql not found; DB clone skipped");
        return;
    };

    if db_exists(&conn, &dst) {
        println!("  db:      {dst} already exists");
        return;
    }
    let src = default_db_name(cfg);
    println!("  db:      cloning {src} -> {dst}");

    // fast path: template clone
    let createdb = Command::new("createdb")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user, "-T", &src, &dst])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if createdb {
        return;
    }

    println!("  db:      template clone failed (source DB in use?); retrying with dump/restore");

    // fallback: plain create + dump/restore
    let plain = Command::new("createdb")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user, &dst])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !plain {
        eprintln!("  db:      create failed; giving up on {dst}");
        return;
    }

    // pg_dump | psql
    let dump = Command::new("pg_dump")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user, "--no-owner", "--no-privileges", &src])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(dump) = dump else {
        eprintln!("  db:      pg_dump failed; {dst} left empty");
        return;
    };
    let restore = Command::new("psql")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user, "-d", &dst, "-q"])
        .stdin(dump.stdout.unwrap())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !restore.map(|s| s.success()).unwrap_or(false) {
        eprintln!("  db:      restore failed; {dst} may be empty");
    }
}

/// Drop the branch's DB. Non-fatal (dcw warns and keeps the DB on failure).
pub fn db_drop(cfg: &Config, branch: &str) {
    let dst = cfg.db_name(branch);
    let Some(conn) = (crate::process::which("dropdb").is_some()).then(|| conn_vars(cfg)) else {
        eprintln!("  db:      dropdb not found; DB drop skipped");
        return;
    };
    if !db_exists(&conn, &dst) {
        println!("  db:      {dst} does not exist");
        return;
    }
    println!("  db:      dropping {dst}");
    let ok = Command::new("dropdb")
        .env("PGPASSWORD", &conn.password)
        .args(["-w", "-h", &conn.host, "-p", &conn.port, "-U", &conn.user, "--if-exists", &dst])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("  db:      warning: dropdb failed for {dst} (left in place)");
    }
}

/// Set/add a KEY=VALUE line in a .env file. Returns (path existed, new value).
fn set_env_line(env_file: &Path, key: &str, value: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(env_file) else {
        return false;
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen = false;
    let target = format!("{key}=");
    for line in text.lines() {
        if line.starts_with(&target) {
            out.push(target.clone() + value);
            seen = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !seen {
        out.push(format!("{key}={value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    std::fs::write(env_file, s).is_ok()
}

/// Point the worktree db.json default.NAME at the branch's DB.
fn set_db_json(cfg: &Config, branch: &str) {
    let dbjson = cfg.dir_for(branch).join("hmx/hmx/db.json");
    let dst = cfg.db_name(branch);
    let Ok(text) = std::fs::read_to_string(&dbjson) else {
        eprintln!("  db.json: {} not found; skipped", dbjson.display());
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Some(def) = v.get_mut("default") {
        def["NAME"] = serde_json::Value::String(dst.clone());
    }
    if let Ok(s) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(&dbjson, s + "\n");
        println!("  db.json: default.NAME = {dst}");
    }
}

/// Point the worktree .env's DATABASE_NAME at the branch DB, and give the worktree a
/// distinct HMX_INSTANCE_NAMESPACE (the Redis isolation key), all in one pass.
pub fn set_worktree_env(cfg: &Config, branch: &str) {
    let env_file = cfg.dir_for(branch).join("hmx/.env");
    let dst = cfg.db_name(branch);

    // Namespace: <main-ns>-<db> (dcw prefixes the main repo's namespace when set).
    let main_ns = env_value(&cfg.repo.join("hmx/.env"), "HMX_INSTANCE_NAMESPACE");
    let ns = if main_ns.is_empty() { dst.clone() } else { format!("{main_ns}-{dst}") };

    if !set_env_line(&env_file, "DATABASE_NAME", &dst) {
        eprintln!("  .env:    {} not found; skipped", env_file.display());
        return;
    }
    println!("  .env:    DATABASE_NAME = {dst}");

    set_env_line(&env_file, "HMX_INSTANCE_NAMESPACE", &ns);
    println!("  .env:    HMX_INSTANCE_NAMESPACE = {ns}");

    set_db_json(cfg, branch);
}

/// Copy the private per-user files/dirs from the main repo into a fresh worktree and
/// git-exclude them so they never get committed.
pub fn copy_private_paths(cfg: &Config, branch: &str) {
    let wt = cfg.dir_for(branch);
    for name in [".agents", ".claude", ".vscode", "AGENTS.md", "CLAUDE.md", "hmx/hmx/db.json"] {
        let src = cfg.repo.join(name);
        let dst = wt.join(name);
        if !src.exists() {
            continue;
        }
        if !dst.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Dir entries (.agents, .claude, .vscode) need a recursive copy —
            // std::fs::copy only handles files and fails silently on a directory.
            match copy_tree(&src, &dst) {
                Ok(()) => println!("  copy:    {}", dst.display()),
                Err(e) => eprintln!("  copy:    warning: {src:?} -> {dst:?} failed: {e}"),
            }
        }
        add_git_exclude(&wt, name);
    }
    sync_claude_md(cfg, &wt);
}

/// Recursive copy (like `cp -a`): files, directories and symlinks.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dst)?;
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    Ok(())
}

/// Mirror $REPO/CLAUDE.md into the worktree (skip-worktree), same as dcw: this clone
/// tracks a symlink, so a fresh worktree would otherwise show /shared's content.
fn sync_claude_md(cfg: &Config, wt: &std::path::Path) {
    let src = cfg.repo.join("CLAUDE.md");
    if !src.is_file() || std::fs::symlink_metadata(&src).map(|m| m.file_type().is_symlink()).unwrap_or(true)
    {
        return;
    }
    let dst = wt.join("CLAUDE.md");
    let identical = dst.is_file()
        && !std::fs::symlink_metadata(&dst).map(|m| m.file_type().is_symlink()).unwrap_or(false)
        && std::fs::read(&src).ok() == std::fs::read(&dst).ok();
    if identical {
        return;
    }
    let _ = std::fs::remove_file(&dst);
    if std::fs::copy(&src, &dst).is_ok() {
        println!("  copy:    {} <- {} (skip-worktree)", dst.display(), src.display());
    }
    add_git_exclude(wt, "CLAUDE.md");
    let _ = crate::git::git_status(wt, &["update-index", "--skip-worktree", "CLAUDE.md"]);
}

/// Add a path to the worktree-local git exclude file (~/.git/info/exclude).
fn add_git_exclude(wt: &Path, name: &str) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(wt)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output();
    let Ok(out) = ok else { return };
    if !out.status.success() {
        return;
    }
    let mut exclude = Path::new(String::from_utf8_lossy(&out.stdout).trim()).to_path_buf();
    if !exclude.is_absolute() {
        exclude = wt.join(exclude);
    }
    if let Ok(text) = std::fs::read_to_string(&exclude) {
        let entry = format!("/{name}");
        if text.lines().any(|l| l == entry) {
            return;
        }
        let mut s = text;
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&entry);
        s.push('\n');
        let _ = std::fs::write(&exclude, s);
    }
}