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
    Start { branch: Option<String> },
    /// Stop server + worker for a branch (keeps worktree/branch)
    Stop { branch: Option<String> },
    /// List all git worktrees with their running/dead state, port, pid, and URL
    List,
    /// Pick a worktree with the arrow keys and print its path (shell function cds there)
    Jump,
    /// Destroy worktree + server + worker + DB (confirmation)
    Abort { branch: Option<String> },
    /// Tail the server or worker log
    Logs { branch: Option<String>, celery: Option<String> },
    /// Manage a branch's Celery worker
    #[command(subcommand)]
    Celery(CeleryCmd),
    /// Fast-forward dcw's own checkout and refresh the binary
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
    Start { branch: Option<String> },
    Restart { branch: Option<String> },
    Stop { branch: Option<String> },
    Status,
}

fn main() {
    let cli = Cli::parse();
    let cfg = config::Config::load();
    let code = match cli.command {
        Commands::New { branch } => commands::create(&cfg, branch),
        Commands::Start { branch } => commands::start(&cfg, branch),
        Commands::Stop { branch } => commands::stop(&cfg, branch),
        Commands::List => {
            commands::list(&cfg);
            0
        }
        Commands::Jump => commands::jump(&cfg),
        Commands::Abort { branch } => commands::abort(&cfg, branch),
        Commands::Logs { branch, celery } => commands::logs(&cfg, branch, celery),
        Commands::Celery(c) => match c {
            CeleryCmd::Start { branch } => {
                let b = branch.unwrap_or_else(|| cfg.base.clone());
                match celery::celery_start(&cfg, &b, true) {
                    Ok(_) => 0,
                    Err(()) => 1,
                }
            }
            CeleryCmd::Restart { branch } => {
                let b = branch.unwrap_or_else(|| cfg.base.clone());
                celery::celery_stop(&cfg, &b);
                match celery::celery_start(&cfg, &b, true) {
                    Ok(_) => 0,
                    Err(()) => 1,
                }
            }
            CeleryCmd::Stop { branch } => {
                let b = branch.unwrap_or_else(|| cfg.base.clone());
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

/// Self-update, faithful to dcw's `update`: locate the checkout from a `.dcw-source`
/// file next to the installed binary (written by the installer), `git pull --ff-only`,
/// rebuild, and atomically replace the running binary.
fn update(_cfg: &config::Config) -> i32 {
    use std::path::PathBuf;

    // Locate the source checkout the binary was installed from.
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("dcw update: cannot resolve own path: {e}");
            return 1;
        }
    };
    let bin_dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    let source_file = bin_dir.join(".dcw-source");
    let Ok(source_dir) = std::fs::read_to_string(&source_file) else {
        eprintln!("dcw update: installation source unknown; run the installer from the checkout once");
        return 1;
    };
    let source_dir = PathBuf::from(source_dir.trim());

    if !source_dir.join("Cargo.toml").is_file() || !source_dir.join("src/main.rs").is_file() {
        eprintln!("dcw update: dcw checkout not found: {}", source_dir.display());
        return 1;
    }
    // The checkout must itself be a git repo root.
    let top = std::process::Command::new("git")
        .arg("-C")
        .arg(&source_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if top != source_dir.to_string_lossy() {
        eprintln!("dcw update: dcw checkout not found: {}", source_dir.display());
        return 1;
    }

    // Refuse to overwrite local edits.
    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(&source_dir)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(true);
    if dirty {
        eprintln!("dcw update: checkout has local changes; save or commit them before updating");
        return 1;
    }

    // Require an upstream, else pull --ff-only has nothing to fast-forward.
    let has_upstream = std::process::Command::new("git")
        .arg("-C")
        .arg(&source_dir)
        .args(["rev-parse", "--verify", "@{upstream}"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_upstream {
        eprintln!("dcw update: branch has no upstream; configure tracking first");
        return 1;
    }

    let old_head = git_rev(&source_dir, "HEAD");
    println!("Checking for dcw updates from {} ...", source_dir.display());
    let pull = std::process::Command::new("git")
        .arg("-C")
        .arg(&source_dir)
        .args(["pull", "--ff-only"])
        .status();
    if !matches!(pull, Ok(s) if s.success()) {
        eprintln!("dcw update: git pull --ff-only failed");
        return 1;
    }
    let new_head = git_rev(&source_dir, "HEAD");
    if old_head == new_head {
        println!("dcw is already up to date. No changes needed.");
        return 0;
    }

    // Rebuild with the toolchain on PATH, then atomically swap the binary.
    eprintln!("Rebuilding dcw from {} ...", source_dir.display());
    let build = std::process::Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(source_dir.join("Cargo.toml"))
        .status();
    if !matches!(build, Ok(s) if s.success()) {
        eprintln!("dcw update: cargo build --release failed");
        return 1;
    }
    let incoming = source_dir.join("target/release/dcw");
    let tmp = bin_dir.join(format!(".dcw.incoming.{}", std::process::id()));
    if std::fs::copy(&incoming, &tmp).is_err() || std::fs::rename(&tmp, &exe).is_err() {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("dcw update: failed to install the new binary");
        return 1;
    }
    println!("dcw updated: {}", &new_head[..new_head.len().min(7)]);
    println!("New commits (subjects):");
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&source_dir)
        .args(["log", "--format=  %h %s", "--reverse", &format!("{old_head}..{new_head}")])
        .output();
    if let Ok(o) = log {
        if !o.stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(&o.stdout));
        }
    }
    0
}

fn git_rev(source_dir: &std::path::Path, rev: &str) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(source_dir)
        .args(["rev-parse", rev])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
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
        let branch = lines.next().unwrap_or(name.trim_end_matches(".pid")).to_string();
        let status = if !process::pid_alive(pid) {
            "dead"
        } else {
            let log = celery::logfile(cfg, &branch);
            if std::fs::read_to_string(&log).map(|s| s.contains(" ready.")).unwrap_or(false) {
                "ready"
            } else {
                "starting"
            }
        };
        let ns = db::env_value(&cfg.dir_for(&branch).join("hmx/.env"), "HMX_INSTANCE_NAMESPACE");
        rows.push((branch.clone(), pid, status.to_string(), ns, celery::logfile(cfg, &branch)));
    }
    if rows.is_empty() {
        println!("(no celery workers)");
        return;
    }
    println!("{:<28} {:<10} {:<10} {:<44} LOG", "BRANCH", "PID", "STATUS", "NS");
    for (branch, pid, status, ns, log) in rows {
        println!("{branch:<28} {pid:<10} {status:<10} {ns:<44} {}", log.display());
    }
}