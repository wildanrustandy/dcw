// progress.rs — a spinner for the silent waits in `dcw new`.
//
// The two slow steps (git fetch, then worktree checkout running alongside the DB
// clone) print nothing until they finish, which reads as a hang. A spinner on stderr
// shows they are alive. Nothing here knows how long a step will take, so it spins
// rather than drawing a percentage.
//
// Every step dcw drives in that window captures its own stdout/stderr
// (git_output/git_status/Command::output), so no child can write over the spinner.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A spinner label. Dropping/stopping it clears its line.
pub struct Spinner {
    handle: Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>,
}

impl Spinner {
    /// Spin on stderr while `label` runs. A no-op when stderr is not a terminal, so a
    /// redirected or piped run never sees the frames.
    pub fn start(label: &str) -> Spinner {
        if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
            return Spinner { handle: None };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let label = label.to_string();
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let start = std::time::Instant::now();
            let mut i = 0usize;
            while !flag.load(Ordering::Relaxed) {
                // Elapsed seconds, not a percentage: dcw cannot know how long a
                // `createdb` or a remote fetch will take, and a fake bar would lie.
                let _ = write!(
                    std::io::stderr(),
                    "\r\x1b[K  {} {label} ({}s)",
                    frames[i % frames.len()],
                    start.elapsed().as_secs()
                );
                let _ = std::io::stderr().flush();
                i += 1;
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        });
        Spinner { handle: Some((stop, handle)) }
    }

    /// Clear the spinner line and stop. Call before printing the step's own result.
    pub fn stop(self) {
        if let Some((stop, handle)) = self.handle {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
            let _ = write!(std::io::stderr(), "\r\x1b[K");
            let _ = std::io::stderr().flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// start/stop must not hang or panic, with or without a terminal attached.
    #[test]
    fn start_then_stop_returns() {
        let spin = Spinner::start("testing");
        std::thread::sleep(std::time::Duration::from_millis(150));
        spin.stop();
    }
}