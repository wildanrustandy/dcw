// setup.rs — first-time init / re-init, mirroring dcw's install.sh questions.
//
//   dcw init                 — show the current config, then (if a config already
//                              exists) ask "Reconfigure? [y/N]" before re-running the
//                              wizard. First run skips the question and goes straight
//                              to the wizard.
//   dcw init --yes           — non-interactive: skip the question and the wizard,
//                              write flags/defaults
//   dcw init --repo PATH --venv PATH --port-base N --port-max N [--bin DIR]
//
// `dcw setup` is an alias for `dcw init`. Output is in English.
//
// Defaults pre-fill in this order: existing ~/.config/dcw/config > existing
// ~/.config/dcw/config (so it mirrors a dcw that was set up first) > built-in
// defaults. The config is written in dcw's shell-default-assignment form, so the
// two tools stay interchangeable to read.

use crate::config::read_config;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// The venv helpers (venvmain / venvhere) shipped with dcw, embedded so `dcw init`
/// can install them without the checkout being present next to the binary.
const VENV_SH: &str = include_str!("../shell/venv.sh");

pub struct InitArgs {
    pub yes: bool,
    pub repo: Option<String>,
    pub venv: Option<String>,
    pub port_base: Option<u16>,
    pub port_max: Option<u16>,
    pub bin: Option<PathBuf>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn config_path() -> PathBuf {
    match std::env::var("DCW_CONFIG") {
        Ok(p) => PathBuf::from(p),
        Err(_) => home().join(".config/dcw/config"),
    }
}

/// The ~/.config/dcw dir that holds the config file and the shipped shell helpers.
fn config_dir() -> PathBuf {
    config_path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| home().join(".config/dcw"))
}

/// Existing values from the dcw config file (used to pre-fill the wizard).
fn existing_values() -> HashMap<String, String> {
    read_config(&config_path())
}

fn ask(prompt: &str, default: &str) -> String {
    let mut out = std::io::stdout();
    write!(out, "{prompt} [{default}]: ").ok();
    out.flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return default.to_string();
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        // expand ~
        if let Some(rest) = trimmed.strip_prefix("~/") {
            format!("{}/{}", home().display(), rest)
        } else if trimmed == "~" {
            home().display().to_string()
        } else {
            trimmed.to_string()
        }
    }
}

fn ask_port(prompt: &str, default: u16) -> u16 {
    loop {
        let answer = ask(prompt, &default.to_string());
        match answer.parse::<u16>() {
            Ok(n) => return n,
            Err(_) => eprintln!("  please enter a number"),
        }
    }
}

/// Ask a yes/no question; defaults to "no". Returns true only for y/yes.
fn confirm(prompt: &str) -> bool {
    let mut out = std::io::stdout();
    write!(out, "{prompt} ").ok();
    out.flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    line.trim().to_lowercase().starts_with('y')
}

/// Print the effective configuration (what dcw will actually use right now).
fn print_current_config() {
    let cfg = crate::config::Config::load();
    let domain = cfg.domain.clone().unwrap_or_else(|| "(localhost)".to_string());
    println!("Current configuration:");
    println!("  repo:       {}", cfg.repo.display());
    println!("  venv:       {}", cfg.venv.display());
    println!("  base:       {}", cfg.base);
    println!("  ports:      {}-{}", cfg.port_base, cfg.port_max);
    println!("  domain:     {domain}");
    println!();
}

pub fn run(args: InitArgs) -> i32 {
    let had_config = config_path().is_file();
    let existing = existing_values();

    // always show the effective config first
    print_current_config();

    // interactive only: if a config already exists and the user passed no value
    // flags, ask before overwriting it.
    let interactive = !args.yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let flags_given =
        args.repo.is_some() || args.venv.is_some() || args.port_base.is_some() || args.port_max.is_some();

    if interactive && had_config && !flags_given {
        if !confirm("Reconfigure? [y/N]") {
            println!("no changes made");
            return 0;
        }
        println!();
    }

    if !interactive {
        println!("(non-interactive: using flags/defaults; run `dcw init` in a terminal to be prompted)");
    }

    // defaults: flag > existing config > built-in
    let def_repo = existing
        .get("DEV_CAPTAIN_REPO")
        .cloned()
        .unwrap_or_else(|| format!("{}/repo", home().display()));
    let def_venv = existing
        .get("DEV_CAPTAIN_VENV")
        .cloned()
        .unwrap_or_else(|| format!("{}/.venv", home().display()));
    let def_pbase = existing
        .get("PORT_BASE")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4200);
    let def_pmax = existing
        .get("PORT_MAX")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4215);

    let repo = match args.repo {
        Some(r) => r,
        None if interactive => ask("HMX project folder (repo root)", &def_repo),
        None => def_repo,
    };
    let venv = match args.venv {
        Some(v) => v,
        None if interactive => ask("Python venv folder", &def_venv),
        None => def_venv,
    };
    let port_base = match args.port_base {
        Some(p) => p,
        None if interactive => ask_port("Base port (base branch server)", def_pbase),
        None => def_pbase,
    };
    let port_max = match args.port_max {
        Some(p) => p,
        None if interactive => ask_port("Max port (end of worktree range)", def_pmax),
        None => def_pmax,
    };

    // validate (warnings only, like dcw)
    let mut warn = 0;
    if !PathBuf::from(&repo).join(".git").is_dir() {
        eprintln!("⚠ warning: {repo} is not a git repo (dcw new will refuse until it is)");
        warn = 1;
    }
    if !PathBuf::from(&venv).join("bin/activate").is_file() {
        eprintln!("⚠ warning: no venv found at {venv} (server will fall back to system python3)");
        warn = 1;
    }
    if port_max <= port_base {
        eprintln!("✗ error: PORT_MAX ({port_max}) must be greater than PORT_BASE ({port_base})");
        return 2;
    }

    // write config in dcw's shell form
    let cfg_dir = config_path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| home().join(".config/dcw"));
    let _ = std::fs::create_dir_all(&cfg_dir);
    let content = format!(
        "# dcw config — generated\n\
         # Precedence: environment variables > this file > built-in defaults\n\
         : \"${{DEV_CAPTAIN_REPO:={repo}}}\"\n\
         : \"${{DEV_CAPTAIN_VENV:={venv}}}\"\n\
         : \"${{PORT_BASE:={port_base}}}\"\n\
         : \"${{PORT_MAX:={port_max}}}\"\n"
    );
    if std::fs::write(config_path(), &content).is_err() {
        eprintln!("✗ error: could not write {}", config_path().display());
        return 1;
    }
    println!("✓ config:   {}", config_path().display());

    // optionally install the binary (like dcw install.sh), recording the source
    if let Some(bin_dir) = args.bin {
        let exe = std::env::current_exe().ok();
        if let Some(exe) = exe {
            let _ = std::fs::create_dir_all(&bin_dir);
            let tmp = bin_dir.join(format!(".dcw.incoming.{}", std::process::id()));
            if std::fs::copy(&exe, &tmp).is_ok() {
                let dest = bin_dir.join("dcw");
                let _ = std::fs::rename(&tmp, &dest);
                println!("✓ installed: {}", dest.display());
            }
            // record the checkout source for `dcw update`
            if let Ok(cwd) = std::env::current_dir() {
                let _ = std::fs::write(bin_dir.join(".dcw-source"), format!("{}\n", cwd.display()));
            }
            // shell integration, so `dcw jump` can cd the CALLING shell
            install_shell_integration(&bin_dir);
        }
    }

    // summary
    println!();
    println!("== summary ==");
    println!("  repo:       {repo}");
    println!("  venv:       {venv}");
    println!("  ports:      {port_base}-{port_max}");
    println!();
    println!("Usage: dcw new <branch> | start <b> | list | jump | logs <b> | stop <b> | abort <b>");
    if warn == 1 {
        println!("(review the warnings above)");
    }
    0
}

/// Write a `dcw` shell function into the rc file(s) of the login shell so that
/// `dcw jump` changes the CALLING shell's directory. A child process cannot cd its
/// parent, so this must be a function — same reason dcw ships one.
fn install_shell_integration(bin_dir: &std::path::Path) {
    let bin = bin_dir.join("dcw");
    let bin = bin.display();
    let marker = "# dcw shell integration (added by dcw init)";
    let body = format!(
        r#"{marker}
# A function, not the binary: `dcw jump` opens the picker and cds into the chosen
# worktree, which a child process cannot do to its parent shell. Every other
# subcommand is passed straight through. `command dcw ...` bypasses this.
dcw() {{
  local _dcw_bin="${{DCW_BIN:-{bin}}}"
  if [ "${{1:-}}" = jump ]; then
    local _dcw_dir
    _dcw_dir=$("$_dcw_bin" jump) || return $?
    [ -n "$_dcw_dir" ] || return 0
    if [ -d "$_dcw_dir/hmx" ]; then
      cd "$_dcw_dir/hmx"
    elif [ -d "$_dcw_dir" ]; then
      cd "$_dcw_dir"
    fi
    return 0
  fi
  "$_dcw_bin" "$@"
}}
"#
    );

    let candidates = rc_candidates();
    let mut written_any = false;
    for rc in &candidates {
        if let Some(parent) = rc.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let existing = std::fs::read_to_string(rc).unwrap_or_default();
        if existing.contains(marker) {
            println!("✓ shell:    dcw function already in {}", rc.display());
            written_any = true;
            continue;
        }
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push('\n');
        updated.push_str(&body);
        if std::fs::write(rc, updated).is_ok() {
            println!("✓ shell:    dcw function added to {}", rc.display());
            written_any = true;
        }
    }
    if !written_any {
        eprintln!("⚠ shell:    could not write shell integration; add it manually if you want `dcw jump`");
    } else {
        println!("            (open a new terminal, or `source` that file, for `dcw jump` to work)");
    }

    install_venv_helpers(&candidates);
}

/// Install shell/venv.sh to ~/.config/dcw/shell/venv.sh and add the guarded
/// `source` line to the rc file(s). A file rather than an inline block: re-running
/// `dcw init` then refreshes the functions without touching the rc file again.
fn install_venv_helpers(rcs: &[PathBuf]) {
    let dest = config_dir().join("shell/venv.sh");
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dest, VENV_SH) {
        Ok(()) => println!("✓ venv:     {} (venvmain, venvhere)", dest.display()),
        Err(e) => {
            eprintln!("⚠ venv:     could not write {}: {e}", dest.display());
            return;
        }
    }

    let marker = "# dcw venv helpers (added by dcw init)";
    let body = format!(
        "{marker}\n[ -f \"$HOME/.config/dcw/shell/venv.sh\" ] && . \"$HOME/.config/dcw/shell/venv.sh\"\n"
    );
    for rc in rcs {
        let existing = std::fs::read_to_string(rc).unwrap_or_default();
        if existing.contains(marker) {
            println!("✓ venv:     already sourced in {}", rc.display());
            continue;
        }
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push('\n');
        updated.push_str(&body);
        if std::fs::write(rc, updated).is_ok() {
            println!("✓ venv:     sourced in {}", rc.display());
        }
    }
}

/// The rc file(s) a NEW terminal actually reads. $SHELL is used as a hint only.
fn rc_candidates() -> Vec<PathBuf> {
    let home = home();
    let shell = std::env::var("SHELL").unwrap_or_default();
    let name = std::path::Path::new(&shell)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.as_str() {
        "zsh" => vec![home.join(".zshenv")],
        "fish" => vec![home.join(".config/fish/config.fish")],
        _ => {
            // bash: ~/.bashrc for interactive shells, plus the first login file
            let mut v = vec![home.join(".bashrc")];
            for f in [".bash_profile", ".bash_login", ".profile"] {
                let p = home.join(f);
                if p.exists() {
                    v.push(p);
                    break;
                }
            }
            v
        }
    }
}

/// Remove dcw from this machine: the installed binary(ies), ~/.config/dcw and the
/// rc-file blocks the installer wrote (each edited rc is backed up first). Worktrees,
/// cloned DBs and the ~/.dev-captain/ state dir are left alone — the same contract
/// as dcw's uninstall.sh. Also cleans the legacy `wtc` names from a pre-rename install.
pub fn uninstall() -> i32 {
    let mut removed = 0;

    // 1. the binary + source marker. The running binary's dir is authoritative; the
    // documented install dir is also cleaned so `dcw` never lingers half-removed.
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    dirs.push(home().join(".local/bin"));
    dirs.sort();
    dirs.dedup();
    for dir in &dirs {
        for name in ["dcw", "wtc", ".dcw-source", ".wtc-source"] {
            let p = dir.join(name);
            if p.exists() && std::fs::remove_file(&p).is_ok() {
                println!("✓ removed {}", p.display());
                removed += 1;
            }
        }
        // leftover atomic-install temp files from an interrupted install
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if (n.starts_with(".dcw.incoming.") || n.starts_with(".wtc.incoming."))
                    && std::fs::remove_file(e.path()).is_ok()
                {
                    println!("✓ removed {}", e.path().display());
                    removed += 1;
                }
            }
        }
    }

    // 2. config dir (written by `dcw init`); legacy ~/.config/wtc too.
    for dir in [config_dir(), home().join(".config/wtc")] {
        if dir.is_dir() && std::fs::remove_dir_all(&dir).is_ok() {
            println!("✓ removed {}", dir.display());
            removed += 1;
        }
    }

    // 3. rc blocks (markers written by our installer, plus the legacy wtc and the
    //    previous dcw installer's markers).
    let rcs = [
        ".zshenv",
        ".bashrc",
        ".bash_profile",
        ".bash_login",
        ".profile",
        ".config/fish/config.fish",
    ];
    for rel in rcs {
        let path = home().join(rel);
        if let Some(n) = strip_rc_blocks(&path) {
            println!(
                "✓ cleaned {n} block(s) from {} (backup: {}.dcw-uninstall.bak)",
                path.display(),
                path.display()
            );
            removed += n;
        }
    }

    if removed == 0 {
        println!("nothing to remove");
    }
    println!("  (worktrees in <repo>/.worktrees, cloned DBs, and ~/.dev-captain/ are left alone)");
    println!("  open a new terminal to drop the dcw shell function");
    0
}

/// An rc-file block: opening marker + predicate matching the line that closes it.
type RcBlock = (&'static str, fn(&str) -> bool);

/// Remove installer-written blocks from an rc file. Returns Some(count) when the
/// file was edited (a `<rc>.dcw-uninstall.bak` backup is written first), None when
/// there is nothing to do.
fn strip_rc_blocks(path: &Path) -> Option<usize> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let blocks: [RcBlock; 6] = [
        ("# dcw shell integration (added by dcw init)", is_closing_brace),
        ("# dcw venv helpers (added by dcw init)", is_venv_source_line),
        ("# dcw shell integration (added by installer)", is_closing_brace),
        ("# dcw venv helpers (added by installer)", is_venv_source_line),
        ("# wtc shell integration (added by wtc init)", is_closing_brace),
        ("# wtc venv helpers (added by wtc init)", is_venv_source_line),
    ];

    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    let mut removed = 0;
    while i < lines.len() {
        match blocks.iter().find(|(m, _)| lines[i].trim_end() == *m) {
            None => {
                out.push(lines[i]);
                i += 1;
            }
            Some((_, end)) => {
                let mut j = i + 1;
                while j < lines.len() && !end(lines[j]) {
                    j += 1;
                }
                i = (j + 1).min(lines.len()); // include the closing line
                removed += 1;
            }
        }
    }
    if removed == 0 {
        return None;
    }

    // collapse the doubled blank line the removal can leave behind
    let mut cleaned: Vec<&str> = Vec::new();
    for l in out {
        if l.trim().is_empty()
            && cleaned.last().map(|p: &&str| p.trim().is_empty()).unwrap_or(false)
        {
            continue;
        }
        cleaned.push(l);
    }

    let _ = std::fs::copy(path, format!("{}.dcw-uninstall.bak", path.display()));
    let mut body = cleaned.join("\n");
    body.push('\n');
    std::fs::write(path, body).ok()?;
    Some(removed)
}

fn is_closing_brace(l: &str) -> bool {
    l.trim_end() == "}"
}

fn is_venv_source_line(l: &str) -> bool {
    l.trim_start().starts_with("[ -f \"$HOME/.config/") && l.contains("venv.sh")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dcw-test-{}-{}", std::process::id(), name));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("rc");
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn strips_current_blocks_and_keeps_the_rest() {
        let body = "\
export FOO=1

# dcw shell integration (added by dcw init)
dcw() {
  echo hi
}

# dcw venv helpers (added by dcw init)
[ -f \"$HOME/.config/dcw/shell/venv.sh\" ] && . \"$HOME/.config/dcw/shell/venv.sh\"
alias x=y
";
        let p = tmp_file("current", body);
        assert_eq!(strip_rc_blocks(&p), Some(2));
        let got = std::fs::read_to_string(&p).unwrap();
        assert!(got.contains("export FOO=1"), "{got}");
        assert!(got.contains("alias x=y"), "{got}");
        assert!(!got.contains("dcw()"), "{got}");
        assert!(!got.contains("venv.sh"), "{got}");
        assert!(std::fs::metadata(format!("{}.dcw-uninstall.bak", p.display())).is_ok());
    }

    #[test]
    fn strips_legacy_wtc_blocks() {
        let body = "\
# wtc shell integration (added by wtc init)
wtc() {
  echo old
}
keep-me
";
        let p = tmp_file("legacy", body);
        assert_eq!(strip_rc_blocks(&p), Some(1));
        let got = std::fs::read_to_string(&p).unwrap();
        assert!(!got.contains("wtc()"), "{got}");
        assert!(got.contains("keep-me"), "{got}");
    }

    #[test]
    fn leaves_unrelated_files_alone() {
        let p = tmp_file("unrelated", "export PATH=/usr/bin\n");
        assert_eq!(strip_rc_blocks(&p), None);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "export PATH=/usr/bin\n");
    }
}