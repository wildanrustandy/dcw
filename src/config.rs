// config.rs — project config, base-branch detection, paths, and small string helpers.
//
// Precedence (same as dcw): environment variables > ~/.config/dcw/config > built-in
// defaults. The config file is KEY=VALUE lines written by the installer; dcw reads it
// into the environment at startup.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

pub struct Config {
    pub repo: PathBuf,
    pub venv: PathBuf,
    pub base: String,
    pub port_base: u16,
    pub port_max: u16,
    pub domain: Option<String>, // None = force http://127.0.0.1:port
    pub user: String,
    pub celery_enabled: bool,
    pub celery_queues: String,
    pub celery_concurrency: u32,
    pub celery_loglevel: String,
    // derived
    #[allow(dead_code)]
    pub config_file: PathBuf,
    #[allow(dead_code)]
    pub state_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub reg_file: PathBuf,
    pub lock_dir: PathBuf,
    pub celery_state_dir: PathBuf,
    pub wt_dir: PathBuf,
}

fn source_config_file() -> HashMap<String, String> {
    let path = env::var("DCW_CONFIG").unwrap_or_else(|_| {
        let mut p = PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/".into()));
        p.push(".config/dcw/config");
        p.to_string_lossy().into_owned()
    });
    read_config(Path::new(&path))
}

/// Read any dcw/dcw-style config file into a name→value map.
pub fn read_config(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            if let Some((k, v)) = parse_assignment(line) {
                map.insert(k, v);
            }
        }
    }
    map
}

/// Parse one config line in either form used by dcw/dcw:
///   KEY=VALUE                     (plain)
///   : "${KEY:=VALUE}"             (dcw's shell default-assignment form)
/// Returns (name, value) with quotes/braces stripped, or None.
fn parse_assignment(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // ':' a bare colon prefix that dcw uses as a no-op shell command is harmless.
    for sep in [":=", "="] {
        if let Some(pos) = line.find(sep) {
            let name = trailing_identifier(&line[..pos])?;
            let value = clean_value(line[pos + sep.len()..].trim().to_string());
            return Some((name, value));
        }
    }
    None
}

/// The identifier (a shell var name) ending right before a `=`/`:=` separator.
/// Scans LEFT from the separator: the identifier is the maximal run of
/// [A-Za-z0-9_] that ends at the separator, so `: "${DEV_CAPTAIN_REPO` → `DEV_CAPTAIN_REPO`.
fn trailing_identifier(prefix: &str) -> Option<String> {
    let bytes = prefix.as_bytes();
    let mut end = bytes.len();
    // trim trailing whitespace before the separator
    while end > 0 && (bytes[end - 1] as char).is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    if start >= end {
        return None; // no identifier before the separator
    }
    let name = &prefix[start..end];
    if name.chars().next().map(|c| c == '_' || c.is_ascii_alphabetic()).unwrap_or(false) {
        Some(name.to_string())
    } else {
        None // starts with a digit — not a valid var name
    }
}

/// Strip surrounding quotes and braces from a value like `/home/wildan2/repo}"`.
fn clean_value(mut v: String) -> String {
    while v.ends_with('}') || v.ends_with('"') || v.ends_with('\'') {
        v.pop();
    }
    while v.starts_with('"') || v.starts_with('\'') || v.starts_with('{') {
        v.drain(..1);
    }
    v
}

/// env override > config file > default. Empty string (from an explicit `VAR=`) is
/// honored, so a cleared value is distinguishable from "unset".
fn resolve(name: &str, config: &HashMap<String, String>, default: &str) -> String {
    if let Ok(v) = env::var(name) {
        return v;
    }
    if let Some(v) = config.get(name) {
        return v.clone();
    }
    default.to_string()
}

fn parse_bool(name: &str, config: &HashMap<String, String>, default: bool) -> bool {
    match resolve(name, config, "").as_str() {
        "1" | "true" | "yes" => true,
        "0" | "false" | "no" => false,
        "" => default,
        other => {
            eprintln!("warning: ignoring bad {name}={other:?}; using {default}");
            default
        }
    }
}

/// Determine the base branch, mirroring dcw: origin/HEAD → branch checked out in the
/// repo → "main", overridable via DEV_CAPTAIN_BASE.
fn detect_base(repo: &Path, config: &HashMap<String, String>) -> String {
    if let Ok(b) = env::var("DEV_CAPTAIN_BASE") {
        if !b.is_empty() {
            return b;
        }
    }
    if let Some(b) = config.get("DEV_CAPTAIN_BASE") {
        if !b.is_empty() {
            return b.clone();
        }
    }
    if repo.join(".git").exists() {
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Some(stripped) = s.strip_prefix("origin/") {
                    return stripped.to_string();
                }
            }
        }
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() && s != "HEAD" {
                    return s;
                }
            }
        }
    }
    "main".to_string()
}

/// Preview-URL domain, same auto-detection as dcw: DCW_DOMAIN > DEVSPACE_DOMAIN >
/// /etc/devspace-nginx-routes.env > none. None means plain http://127.0.0.1:port.
fn detect_domain(config: &HashMap<String, String>) -> Option<String> {
    // DCW_DOMAIN empty-string forces localhost: distinguish "set" from "unset".
    if let Ok(v) = env::var("DCW_DOMAIN") {
        return if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = config.get("DCW_DOMAIN") {
        return if v.is_empty() { None } else { Some(v.clone()) };
    }
    if let Ok(v) = env::var("DEVSPACE_DOMAIN") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    let routes = Path::new("/etc/devspace-nginx-routes.env");
    if routes.is_file() {
        if let Ok(text) = std::fs::read_to_string(routes) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == "DEVSPACE_DOMAIN" {
                        let v = v.trim();
                        if !v.is_empty() {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

impl Config {
    pub fn load() -> Config {
        let config = source_config_file();
        let home = env::var("HOME").unwrap_or_else(|_| "/".into());
        let home = PathBuf::from(home);

        let repo = PathBuf::from(resolve(
            "DEV_CAPTAIN_REPO",
            &config,
            &format!("{}/dev/hmx-002", home.display()),
        ));
        let venv = PathBuf::from(resolve(
            "DEV_CAPTAIN_VENV",
            &config,
            &format!("{}/dev/.venv", home.display()),
        ));

        let port_base = resolve("PORT_BASE", &config, "5200").parse::<u16>().unwrap_or(5200);
        let port_max = resolve("PORT_MAX", &config, "5220").parse::<u16>().unwrap_or(5220);

        let base = detect_base(&repo, &config);
        let domain = detect_domain(&config);
        let user = resolve("DCW_USER", &config, env::var("USER").as_deref().unwrap_or("user"));

        let celery_enabled = parse_bool("DCW_CELERY", &config, true);
        let celery_queues = resolve(
            "DCW_CELERY_QUEUES",
            &config,
            "interactive,import,export,scheduled,notifications,csv,data_import,demo_pool,celery",
        );
        let celery_concurrency = resolve("DCW_CELERY_CONCURRENCY", &config, "2")
            .parse::<u32>()
            .unwrap_or(2);
        let celery_loglevel = resolve("DCW_CELERY_LOGLEVEL", &config, "INFO");

        let state_dir = home.join(".dev-captain");
        let wt_dir = repo.join(".worktrees");

        let config_file = env::var("DCW_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config/dcw/config"));

        Config {
            repo,
            venv,
            base,
            port_base,
            port_max,
            domain,
            user,
            celery_enabled,
            celery_queues,
            celery_concurrency,
            celery_loglevel,
            config_file,
            logs_dir: state_dir.join("logs"),
            reg_file: state_dir.join("ports.json"),
            lock_dir: state_dir.join(".lock"),
            celery_state_dir: state_dir.join("celery"),
            state_dir,
            wt_dir,
        }
    }

    /// Preview URL for a branch, mirroring dcw's _url_of.
    pub fn url_for(&self, port: u16, branch: &str) -> String {
        match &self.domain {
            None => format!("http://127.0.0.1:{port}"),
            Some(domain) => {
                if branch == self.base {
                    format!("https://{}.{domain}", self.user)
                } else {
                    format!("https://{}-{port}.{domain}", self.user)
                }
            }
        }
    }

    /// Directory a branch lives in (base branch → repo itself, else the worktree).
    pub fn dir_for(&self, branch: &str) -> PathBuf {
        if branch == self.base {
            self.repo.clone()
        } else {
            self.wt_dir.join(branch)
        }
    }

    /// postgres db name for a branch: lowercase, safe chars, ≤63 chars (dcw `_db_name_for_branch`).
    pub fn db_name(&self, branch: &str) -> String {
        let mut out = String::new();
        for c in branch.chars() {
            match c {
                '/' | '-' | '.' => out.push('_'),
                c if c.is_ascii_uppercase() => out.extend(c.to_lowercase()),
                c if c.is_ascii_alphanumeric() || c == '_' => out.push(c),
                _ => {} // drop unsafe chars
            }
        }
        if out.len() > 63 {
            out.truncate(63);
        }
        out
    }

    /// slug for file names: '/' → '_' (dcw `_slug`).
    pub fn slug(&self, branch: &str) -> String {
        branch.replace('/', "_")
    }
}