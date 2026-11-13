use clap::{Args, Parser, Subcommand};

mod celery;
mod commands;
mod config;
mod db;
mod git;
mod picker;
mod process;
mod registry;
mod setup;

#[derive(Parser)]
#[command(name = "dcw", version, about = "worktree + port + Django server + Celery lifecycle")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a NEW worktree (clone DB, copy private files, set Redis namespace) and start it
    New { branch: Option<String> },
    /// Start server + worker for an EXISTING worktree (never creates one)
    Start {
        /// worktree name (its directory); a branch name resolves to its worktree
        worktree: Option<String>,
    },
    /// Stop server + worker for a worktree (keeps worktree/branch)
    Stop {
        /// worktree name (its directory); a branch name resolves to its worktree
        worktree: Option<String>,
    },
    /// Stop then start a worktree's server + worker (keeps worktree, DB, and port)
    Restart {
        /// worktree name (its directory); a branch name resolves to its worktree
        worktree: Option<String>,
    },
    /// List all git worktrees with their running/dead state, port, pid, and URL
    List,
    /// Pick a worktree with the arrow keys and print its path (shell function cds there)
    Jump,
    /// Destroy worktree + server + worker + DB (confirmation)
    Abort {
        /// worktree name (its directory); a branch name resolves to its worktree
        worktree: Option<String>,
    },
    /// Tail the server or worker log
    Logs {
        /// worktree name (its directory); a branch name resolves to its worktree
        worktree: Option<String>,
        /// "celery" to tail the worker log instead of the server log
        celery: Option<String>,
    },
    /// Manage a worktree's Celery worker
    #[command(subcommand)]
    Celery(CeleryCmd),
    /// Download the latest prebuilt release and replace the running binary
    Update,
    /// Re-initialize: show current config, ask to reconfigure, then install (like dcw install.sh)
    #[command(alias = "setup")]
    Init(InitArgs),
    /// Remove dcw from this machine (binary, config, rc-file blocks); keeps worktrees/DBs
    Uninstall,
}

#[derive(Args)]
struct InitArgs {
    /// accept defaults non-interactively
    #[arg(long, short = 'y')]
    yes: bool,
    /// HMX project root
    #[arg(long)]
    repo: Option<String>,
    /// python venv path
    #[arg(long)]
    venv: Option<String>,
    /// base port for the base branch
    #[arg(long)]
    port_base: Option<u16>,
    /// upper bound of the port range
    #[arg(long)]
    port_max: Option<u16>,
    /// install the binary into DIR (and record the source)
    #[arg(long)]
    bin: Option<String>,
}

#[derive(Subcommand)]
enum CeleryCmd {
    Start { worktree: Option<String> },
    Restart { worktree: Option<String> },
    Stop { worktree: Option<String> },
    Status,
}

/// Resolve a user-supplied worktree name (or branch alias) to the key dcw files
/// everything under: the worktree's directory name. Falls back to the raw argument
/// (then the base branch) when it does not match a known worktree.
fn resolve_key(cfg: &config::Config, arg: Option<String>) -> String {
    match arg {
        Some(a) => match commands::find_target(cfg, &a) {
            Some(t) => t.name,
            None => a,
        },
        None => cfg.base.clone(),
    }
}

fn main() {
    let cli = Cli::parse();
    let cfg = config::Config::load();
    let code = match cli.command {
        Commands::New { branch } => commands::create(&cfg, branch),
        Commands::Start { worktree } => commands::start(&cfg, worktree),
        Commands::Stop { worktree } => commands::stop(&cfg, worktree),
        Commands::Restart { worktree } => commands::restart(&cfg, worktree),
        Commands::List => {
            commands::list(&cfg);
            0
        }
        Commands::Jump => commands::jump(&cfg),
        Commands::Abort { worktree } => commands::abort(&cfg, worktree),
        Commands::Logs { worktree, celery } => commands::logs(&cfg, worktree, celery),
        Commands::Celery(c) => match c {
            CeleryCmd::Start { worktree } => {
                let b = resolve_key(&cfg, worktree);
                match celery::celery_start(&cfg, &b, true) {
                    Ok(_) => 0,
                    Err(()) => 1,
                }
            }
            CeleryCmd::Restart { worktree } => {
                let b = resolve_key(&cfg, worktree);
                celery::celery_stop(&cfg, &b);
                match celery::celery_start(&cfg, &b, true) {
                    Ok(_) => 0,
                    Err(()) => 1,
                }
            }
            CeleryCmd::Stop { worktree } => {
                let b = resolve_key(&cfg, worktree);
                celery::celery_stop(&cfg, &b);
                0
            }
            CeleryCmd::Status => {
                celery_status_all(&cfg);
                0
            }
        },
        Commands::Update => update(&cfg),
        Commands::Uninstall => setup::uninstall(),
        Commands::Init(args) => setup::run(setup::InitArgs {
            yes: args.yes,
            repo: args.repo,
            venv: args.venv,
            port_base: args.port_base,
            port_max: args.port_max,
            bin: args.bin.map(std::path::PathBuf::from),
        }),
    };
    std::process::exit(code);
}

/// Self-update for a curl-installed binary: download the latest prebuilt release for
/// this OS/arch and atomically replace the running binary. No git checkout and no Rust
/// toolchain are needed on the target machine.
///
/// The download URL is resolved from, in order:
///   1. `DCW_UPDATE_URL`  — the full URL of the binary (hosting override)
///   2. `DCW_UPDATE_BASE` — a base URL the per-platform asset name is appended to
///   3. the built-in GitHub latest-release download for this project
fn update(_cfg: &config::Config) -> i32 {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("dcw update: cannot resolve own path: {e}");
            return 1;
        }
    };
    let bin_dir = match exe.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            eprintln!("dcw update: cannot determine install directory");
            return 1;
        }
    };

    let url = update_url();
    let tmp = bin_dir.join(format!(".dcw.incoming.{}", std::process::id()));
    println!("Downloading dcw from {url} ...");
    if !download(&url, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        eprintln!(
            "dcw update: download failed (see above); re-run the installer or set DCW_UPDATE_URL"
        );
        return 1;
    }

    // Make it executable and read its version (best effort) before swapping.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    let new_version = std::process::Command::new(&tmp)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    if std::fs::rename(&tmp, &exe).is_err() {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("dcw update: failed to replace the running binary");
        return 1;
    }

    if new_version.is_empty() {
        println!("dcw updated.");
    } else {
        println!("dcw updated ({new_version}).");
    }
    0
}

/// Resolve the download URL for this platform's prebuilt binary. Asset names are
/// `dcw-<os>-<arch>` using Rust's `std::env::consts` values (e.g. `dcw-linux-x86_64`),
/// which `install.sh` reproduces from `uname`.
fn update_url() -> String {
    if let Ok(full) = std::env::var("DCW_UPDATE_URL") {
        if !full.is_empty() {
            return full;
        }
    }
    let asset = format!("dcw-{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let base = std::env::var("DCW_UPDATE_BASE").unwrap_or_else(|_| {
        "https://github.com/wildanrustandy/dcw/releases/latest/download"
            .to_string()
    });
    format!("{}/{}", base.trim_end_matches('/'), asset)
}

/// Download `url` to `dest`, preferring curl and falling back to wget. Returns true
/// only when a client reported success AND a non-empty file landed at `dest`.
fn download(url: &str, dest: &std::path::Path) -> bool {
    let curl = std::process::Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(dest)
        .arg(url)
        .status();
    if matches!(curl, Ok(s) if s.success() && file_nonempty(dest)) {
        return true;
    }
    let wget = std::process::Command::new("wget")
        .args(["-q", "-O"])
        .arg(dest)
        .arg(url)
        .status();
    matches!(wget, Ok(s) if s.success() && file_nonempty(dest))
}

fn file_nonempty(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

fn celery_status_all(cfg: &config::Config) {
    use crate::process;
    let mut rows = Vec::new();
    let Ok(entries) = std::fs::read_dir(&cfg.celery_state_dir) else {
        println!("(no celery workers)");
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".pid") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(cfg.celery_state_dir.join(&name)) else { continue };
        let mut lines = text.lines();
        let Some(pid) = lines.next().and_then(|l| l.trim().parse::<i32>().ok()) else { continue };
        // line 2 is the worktree name the worker belongs to (dcw's identity)
        let worktree = lines.next().unwrap_or(name.trim_end_matches(".pid")).to_string();
        let status = if !process::pid_alive(pid) {
            "dead"
        } else {
            let log = celery::logfile(cfg, &worktree);
            if std::fs::read_to_string(&log).map(|s| s.contains(" ready.")).unwrap_or(false) {
                "ready"
            } else {
                "starting"
            }
        };
        let ns = db::env_value(&cfg.dir_for(&worktree).join("hmx/.env"), "HMX_INSTANCE_NAMESPACE");
        rows.push((worktree.clone(), pid, status.to_string(), ns, celery::logfile(cfg, &worktree)));
    }
    if rows.is_empty() {
        println!("(no celery workers)");
        return;
    }
    println!("{:<28} {:<10} {:<10} {:<44} LOG", "WORKTREE", "PID", "STATUS", "NS");
    for (worktree, pid, status, ns, log) in rows {
        println!("{worktree:<28} {pid:<10} {status:<10} {ns:<44} {}", log.display());
    }
}