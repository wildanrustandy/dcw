// ram.rs — `dcw ram`: where the current user's RAM goes, dcw-managed worktrees first.
//
// One `ps` snapshot is classified against what dcw already tracks: the Django pid in
// ports.json (server) and the Celery pids in ~/.dev-captain/celery/ (worker). Because
// process::spawn_detached() calls setsid (PGID == PID), summing a whole process group
// also catches the runserver autoreload child and the prefork Celery children.
// Everything else the user runs is grouped by command name.
//
// RSS sums double-count pages shared between processes (Celery prefork children are
// copy-on-write copies), so read the numbers as "which worktree is heavy", not
// byte-exact accounting — PSS would be exact but needs smem, which is not installed
// by default.

use crate::commands;
use crate::config::Config;
use crate::registry;
use std::collections::HashMap;

pub fn ram(cfg: &Config) {
    let user = match std::process::Command::new("whoami").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            eprintln!("dcw ram: whoami failed; cannot determine the current user");
            return;
        }
    };
    let ps = match std::process::Command::new("ps")
        .args(["-u", &user, "-o", "pgid=,rss=,args="])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => {
            eprintln!("dcw ram: ps failed");
            return;
        }
    };

    // Tracked process groups. Registry pids are the Django servers, celery/*.pid the
    // workers; PGID == PID for both, so a ps row lands on its worktree by group.
    let mut servers: HashMap<i32, String> = HashMap::new();
    for info in registry::load(&cfg.reg_file).values() {
        if info.pid > 0 && !info.branch.is_empty() {
            servers.insert(info.pid, info.branch.clone());
        }
    }
    let mut workers: HashMap<i32, String> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.celery_state_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".pid") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            let mut lines = text.lines();
            let Some(pid) = lines.next().and_then(|l| l.trim().parse::<i32>().ok()) else {
                continue;
            };
            // line 2 is the worktree name (dcw's identity); older files carry only the pid
            let wt = lines
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(name.trim_end_matches(".pid"))
                .to_string();
            workers.insert(pid, wt);
        }
    }

    let (worktrees, others, total) = classify(&ps, &servers, &workers);
    if worktrees.is_empty() && others.is_empty() {
        println!("(no processes for user {user})");
        return;
    }
    let human = |kb: u64| -> String {
        // ps rss is in KB; show MB until a full GiB reads cleaner than "1048576 MB"
        if kb < 1024 * 1024 {
            format!("{} MB", kb / 1024)
        } else {
            format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0))
        }
    };
    let dcw_total: u64 = worktrees.iter().map(|(_, s, w)| s + w).sum();
    if worktrees.is_empty() {
        println!("(no dcw-managed processes)");
    } else {
        let rows: Vec<Vec<String>> = worktrees
            .iter()
            .map(|(n, s, w)| vec![n.clone(), human(*s), human(*w), human(*s + *w)])
            .collect();
        commands::render_box(&["WORKTREE", "SERVER", "WORKER", "TOTAL"], &rows);
    }

    if !others.is_empty() {
        println!("\nOther processes (not managed by dcw):");
        let rows: Vec<Vec<String>> = others
            .iter()
            .map(|(comm, kb)| vec![comm.clone(), human(*kb)])
            .collect();
        commands::render_box(&["COMMAND", "RSS"], &rows);
    }
    println!("\ndcw total: {}", human(dcw_total));
    println!("user {user} total: {}", human(total));
}

/// Sum RSS by process group: groups whose pgid is a tracked server/worker pid land in
/// that worktree's column; the rest are grouped by command name. Pure — the test feeds
/// it synthetic `ps` output.
/// (worktree, server KB, worker KB) rows, (command, KB) rows, and the user's total KB.
type Classified = (Vec<(String, u64, u64)>, Vec<(String, u64)>, u64);

fn classify(
    ps: &str,
    servers: &HashMap<i32, String>,
    workers: &HashMap<i32, String>,
) -> Classified {
    let mut per_worktree: HashMap<String, (u64, u64)> = HashMap::new();
    let mut other: HashMap<String, u64> = HashMap::new();
    let mut total = 0u64;
    for line in ps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pgid), Some(rss)) = (fields.next(), fields.next()) else { continue };
        let (Ok(pgid), Ok(rss)) = (pgid.parse::<i32>(), rss.parse::<u64>()) else { continue };
        // Label = basename of argv[0]; comm is truncated to 15 chars and node/electron
        // apps rename it ("MainThread"). Generic runtimes (node, python) hide the real
        // app behind one name, so their label is the first non-flag argument — the
        // script being run. Paths containing spaces split wrong — fine here.
        // "postgres: writer" and bracketed kernel-style names collapse to the bare name.
        let clean = |s: &str| {
            let base = s.rsplit('/').next().unwrap_or(s);
            base.trim_end_matches(':').trim_matches(|c| c == '[' || c == ']').to_string()
        };
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(exe) = fields.get(2) else { continue };
        let base = clean(exe);
        let label = if matches!(base.as_str(), "node" | "python" | "python3") {
            fields
                .iter()
                .skip(3)
                .find(|a| !a.starts_with('-'))
                .map(|s| clean(s))
                .unwrap_or_else(|| base.clone())
        } else {
            base
        };
        total += rss;
        if let Some(name) = servers.get(&pgid) {
            per_worktree.entry(name.clone()).or_default().0 += rss;
        } else if let Some(name) = workers.get(&pgid) {
            per_worktree.entry(name.clone()).or_default().1 += rss;
        } else {
            *other.entry(label).or_default() += rss;
        }
    }
    let mut worktrees: Vec<(String, u64, u64)> =
        per_worktree.into_iter().map(|(n, (s, w))| (n, s, w)).collect();
    worktrees.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then(a.0.cmp(&b.0)));
    let mut others: Vec<(String, u64)> = other.into_iter().collect();
    others.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    (worktrees, others, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_tracked_pgid_then_by_argv0() {
        let mut servers = HashMap::new();
        servers.insert(100, "PRD_INV_1".to_string());
        let mut workers = HashMap::new();
        workers.insert(200, "PRD_INV_1".to_string());
        // pgid 100 = tracked server; pgid 200 = tracked worker (twice: master + child);
        // pgid 300+ = unrelated processes, labeled by argv[0] basename — except generic
        // runtimes, which are labeled by their first non-flag argument (the script).
        let ps = "  100  500 /venv/bin/python manage.py runserver\n  200  300 /venv/bin/celery -A hmx worker\n  200  100 /venv/bin/celery -A hmx worker\n  300  700 /snap/firefox/1234/firefox -contentproc\n  301   90 postgres: writer process\n  302   10 [orca-ide]\n  400  100 /usr/bin/node /orca/orcad-server.js --bind 127.0.0.1\n  401   50 /usr/bin/node --max-old-space-size=384 daemon-entry.js\n  402   10 /usr/bin/python3 -m http.server\n";
        let (worktrees, others, total) = classify(ps, &servers, &workers);
        assert_eq!(worktrees, vec![("PRD_INV_1".to_string(), 500, 400)]);
        assert_eq!(
            others,
            vec![
                ("firefox".to_string(), 700),
                ("orcad-server.js".to_string(), 100),
                ("postgres".to_string(), 90),
                ("daemon-entry.js".to_string(), 50),
                ("http.server".to_string(), 10),
                ("orca-ide".to_string(), 10),
            ]
        );
        assert_eq!(total, 1860);
    }
}
