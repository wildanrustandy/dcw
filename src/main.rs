use clap::{Args, Parser, Subcommand};

mod celery;
mod changelog;
mod commands;
mod config;
mod db;
mod git;
mod picker;
mod process;
mod progress;
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
    /// Create NEW worktree(s) (clone DB, copy private files, set Redis namespace) and start them.
    /// One name, or several: `dcw new a b` / `dcw new a,b,c`
    New {
        #[arg(value_delimiter = ',')]
        branches: Vec<String>,
    },
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
    /// Print release notes (all of them, or only those newer than a version)
    Changelog {
        /// show only releases newer than this one (e.g. the version you are running)
        since: Option<String>,
    },
    /// Show the effective config and change settings interactively (Celery autostart)
    Config,
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
        Commands::New { branches } => commands::create(&cfg, branches),
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
                match celery::celery_start(&cfg, &b, true, false) {
                    Ok(_) => 0,
                    Err(()) => 1,
                }
            }
            CeleryCmd::Restart { worktree } => {
                let b = resolve_key(&cfg, worktree);
                celery::celery_stop_print(&cfg, &b, 2);
                match celery::celery_start(&cfg, &b, true, false) {
                    Ok(_) => 0,
                    Err(()) => 1,
                }
            }
            CeleryCmd::Stop { worktree } => {
                let b = resolve_key(&cfg, worktree);
                celery::celery_stop_print(&cfg, &b, 2);
                0
            }
            CeleryCmd::Status => {
                celery_status_all(&cfg);
                0
            }
        },
        Commands::Update => update(&cfg),
        Commands::Changelog { since } => changelog_cmd(since),
        Commands::Uninstall => setup::uninstall(),
        Commands::Config => {
            setup::config_menu();
            0
        }
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

/// `dcw changelog [version]`: all notes, or only those newer than `version`. The latter is
/// what `dcw update` runs on the freshly installed binary, so a jump across several
/// releases shows each of them.
fn changelog_cmd(since: Option<String>) -> i32 {
    let Some(from) = since else {
        return match changelog::latest() {
            Some((version, body)) => {
                println!("{version}\n\n{body}");
                0
            }
            None => {
                eprintln!("dcw changelog: this build has no release notes");
                1
            }
        };
    };
    let from = changelog::parse_version(&from).unwrap_or(from);
    match changelog::since(&from) {
        // Empty output (not a message) so `dcw update` can tell "no range to show" from
        // "nothing changed" and fall back to the newest entry.
        Some(notes) if notes.is_empty() => {
            eprintln!("dcw: no releases newer than {from}.");
            0
        }
        Some(notes) => {
            println!("{notes}");
            0
        }
        None => {
            eprintln!("dcw changelog: {from} is not a version in the changelog (typo?)");
            1
        }
    }
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
    // macOS: Gatekeeper quarantines a binary fetched by curl and kills the unsigned
    // result on first run. install.sh strips the attribute; do the same here, else the
    // freshly swapped dcw would be unrunnable.
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(&tmp)
        .status();
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

    match new_version.is_empty() {
        true => println!("dcw updated."),
        false => println!("dcw updated ({new_version})."),
    }
    print_release_notes(&exe, &new_version);
    0
}

/// Show what changed, by asking the NEW binary for its own notes: this process's embedded
/// changelog predates the release it just installed, so it cannot describe it. Best effort
/// — a binary that cannot answer (a platform quirk, a missing subcommand) must not turn a
/// successful update into a failure.
fn print_release_notes(exe: &std::path::Path, new_version: &str) {
    // Re-running `dcw update` on the latest release has nothing to report.
    let current = env!("CARGO_PKG_VERSION");
    if changelog::parse_version(new_version).as_deref() == Some(current) {
        return;
    }
    let ask = |since: Option<&str>| -> String {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("changelog");
        if let Some(s) = since {
            cmd.arg(s);
        }
        match cmd.output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        }
    };
    // Everything between the version that ran and the one just installed. Falling back to
    // the newest entry covers a version the changelog does not know (older than its oldest
    // entry), where there is no range to describe.
    let mut notes = ask(Some(current));
    if notes.is_empty() {
        notes = ask(None);
    }
    if !notes.is_empty() {
        println!("\nWhat's new:\n\n{notes}");
    }
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