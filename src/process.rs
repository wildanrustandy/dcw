// process.rs — daemon spawn, kill, zombie detection, and port-binding checks.
//
// dcw relies on `setsid nohup CMD ... &` so a dying terminal cannot kill the child.
// In Rust the same is done in-process with pre_exec: setsid() + ignore SIGHUP, so no
// external setsid/nohup binary is needed. Zombie detection reads /proc/<pid>/stat's
// state letter, since kill(pid, 0) succeeds for a zombie.

use nix::sys::signal::{kill, Signal};
use nix::unistd::{setsid, Pid};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Spawn a background command that outlives its parent terminal.
///
/// `log` is opened for append and used for both stdout and stderr (dcw redirects
/// `> log 2>&1`). `dir` is the working directory. Returns the child's PID.
pub fn spawn_detached(dir: &Path, log: &Path, program: &str, args: &[String]) -> std::io::Result<i32> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    let file = file; // moved into stdio

    // The file must be cloned for both stdout and stderr; Stdio::from clones the fd.
    let stdout = file.try_clone()?;
    let stderr = file;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        cmd.pre_exec(|| {
            // New session: session leader dies without signaling us; ignore SIGHUP so
            // a terminal hangup never kills the child.
            let _ = setsid();
            let _ = nix::sys::signal::signal(Signal::SIGHUP, nix::sys::signal::SigHandler::SigIgn);
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(child.id() as i32)
}

/// Is pid alive? Returns false for dead, zombie, and non-existent processes.
///
/// Linux reads /proc/<pid>/stat's state letter: a zombie ("Z") counts as dead, since
/// kill(pid, 0) succeeds for zombies. macOS/BSD has no procfs, so it falls back to
/// kill(pid, 0): ESRCH means gone; zombies there are rare and auto-reaped.
pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                // state is field 3; it follows the ")" that ends the comm field (comm
                // may contain spaces/parens, so split on the last ") ").
                let state = stat.rsplit(") ").next().and_then(|rest| rest.split(' ').next());
                match state {
                    Some("Z") | Some("X") | None => false,
                    Some(_) => true,
                }
            }
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        match kill(Pid::from_raw(pid), None) {
            Ok(_) => true,
            Err(nix::errno::Errno::ESRCH) => false,
            Err(_) => true, // e.g. EPERM: process exists but not owned by us
        }
    }
}

/// Is a TCP port bound by any process (macOS-style lsof check)?
pub fn port_bound(port: u16) -> bool {
    let out = Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(out, Ok(s) if s.success())
}

/// Send a signal to a pid, ignoring errors (pid already gone).
pub fn signal(pid: i32, sig: Signal) {
    let _ = kill(Pid::from_raw(pid), sig);
}

/// Stop the process group led by `pid`: SIGTERM the leader, wait up to `grace_secs`,
/// then SIGKILL the whole group.
///
/// The group is the pid itself: every child is spawned through `spawn_detached`, which
/// calls setsid() in pre_exec, making the child its own session and group leader. A
/// celery worker's forked children live in that group too, so signalling the leader
/// alone can leave them behind; signalling the group catches them. `grace_secs = 0`
/// skips the graceful window.
pub fn stop_group(pid: i32, grace_secs: u64) {
    if pid <= 0 {
        return;
    }
    signal(pid, Signal::SIGTERM);
    for _ in 0..(grace_secs * 10) {
        if !pid_alive(pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Always signal the group, even when the leader exited: its children (celery
    // forks) can outlive it and would keep the port/queue alive.
    let _ = kill(Pid::from_raw(-pid), Signal::SIGKILL);
    signal(pid, Signal::SIGKILL);
}

/// The venv's `activate`-provided environment is not needed here: dcw sources it to
/// put the venv's bin dir on PATH. We resolve binaries from the venv directory
/// directly, so a helper returns the path to a venv binary if it exists.
pub fn venv_bin(venv: &Path, name: &str) -> Option<PathBuf> {
    let p = venv.join("bin").join(name);
    if p.is_file() {
        Some(p)
    } else {
        which(name)
    }
}

/// Find a binary on PATH (None if absent).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A leader that ignores SIGTERM with a child that also ignores it must be gone
    /// after stop_group: the group SIGKILL is what catches them (celery's forked
    /// children outlive a leader-only kill).
    #[test]
    fn stop_group_kills_leader_and_children() {
        let dir = std::env::temp_dir().join(format!("dcw-stopgroup-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let child_pidfile = dir.join("child.pid");
        // sh traps TERM, forks a trapping child that reports its pid, then waits.
        let script = format!(
            "trap '' TERM; sh -c 'trap \"\" TERM; echo $$ > {}; while :; do sleep 1; done' & wait",
            child_pidfile.display()
        );
        let pid = spawn_detached(&dir, &dir.join("log"), "sh", &["-c".to_string(), script]).unwrap();
        let mut child = 0;
        for _ in 0..50 {
            if let Ok(t) = std::fs::read_to_string(&child_pidfile) {
                if let Ok(p) = t.trim().parse::<i32>() {
                    child = p;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(pid_alive(pid) && child > 0, "probe tree did not start");

        stop_group(pid, 0);

        // SIGKILL is asynchronous: give the kernel a moment to reap both.
        let gone = |p: i32| {
            for _ in 0..50 {
                if !pid_alive(p) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        };
        assert!(gone(pid), "leader survived stop_group");
        assert!(gone(child), "child survived stop_group");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
