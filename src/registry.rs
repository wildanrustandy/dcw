// registry.rs — the port registry (~/.dev-captain/ports.json), the single source of truth
// for port ↔ branch ↔ pid, plus an flock-based lock (released automatically when the
// holding process dies, so a killed dcw can never leave the lock stuck).
//
// Schema (same shape as dcw's ~/.dev-captain/ports.json):
//   { "<port>": { "branch": "...", "pid": 0, "started_at": <f64> } }

use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct PortInfo {
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub pid: i32,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
}

pub type Registry = BTreeMap<String, PortInfo>;

pub fn load(reg: &Path) -> Registry {
    match std::fs::read_to_string(reg) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(data) => data,
            // Never silently drop the registry: a corrupt file read as empty would be
            // written back as empty on the next save, losing every tracked port.
            Err(e) => {
                eprintln!(
                    "warning: {} is not valid JSON ({e}); ignoring it — ports.json intact",
                    reg.display()
                );
                Registry::new()
            }
        },
        Err(_) => Registry::new(),
    }
}

/// Write the registry atomically: a truncated in-place write can lose the whole file on
/// a crash, so write a temp file in the same dir and rename it over the target.
pub fn save(reg: &Path, data: &Registry) {
    let Some(parent) = reg.parent() else { return };
    let _ = std::fs::create_dir_all(parent);
    let Ok(json) = serde_json::to_string_pretty(data) else {
        return;
    };
    let tmp = parent.join(format!(".ports.json.tmp.{}", std::process::id()));
    let write = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(json.as_bytes()));
    if write.is_err() || std::fs::rename(&tmp, reg).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Serializes ports.json writers. Backed by flock on `<lock_dir>/ports.lock`, so the
/// kernel drops the lock when the holder exits — no stale lock to clean up by hand.
pub struct Lock {
    _file: Option<Flock<File>>,
}

impl Lock {
    /// Everything lives in `dir` (cfg.lock_dir); the lock file is created inside it.
    pub fn acquire(dir: PathBuf) -> Lock {
        let path = dir.join("ports.lock");
        let open = std::fs::create_dir_all(&dir).and_then(|_| File::create(&path));
        match open {
            Ok(file) => match Flock::lock(file, FlockArg::LockExclusive) {
                Ok(held) => Lock { _file: Some(held) },
                Err(_) => {
                    eprintln!("warning: could not lock {}; proceeding without", path.display());
                    Lock { _file: None }
                }
            },
            Err(e) => {
                eprintln!("warning: could not lock {} ({e}); proceeding without", path.display());
                Lock { _file: None }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dcw-regtest-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn save_load_round_trip() {
        let path = tmp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let mut reg = Registry::new();
        reg.insert(
            "5201".into(),
            PortInfo { branch: "PRD_INV_1".into(), pid: 4242, started_at: Some(1.5) },
        );
        save(&path, &reg);
        let back = load(&path);
        assert_eq!(back.get("5201").map(|i| (i.branch.as_str(), i.pid)), Some(("PRD_INV_1", 4242)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let path = tmp_path("notmp");
        let _ = std::fs::remove_file(&path);
        save(&path, &Registry::new());
        let leftovers = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".ports.json.tmp."))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_file(&path);
    }

    // A corrupt file must not be read as an empty registry AND rewritten as empty —
    // load() only reports it; the bytes on disk stay untouched.
    #[test]
    fn load_keeps_a_corrupt_file_untouched() {
        let path = tmp_path("corrupt");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load(&path).is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = std::fs::remove_file(&path);
    }

    // The old mkdir lock stayed held forever after a killed dcw. flock is dropped by the
    // kernel on exit, so acquiring must exclude while held and release on drop.
    #[test]
    fn lock_excludes_while_held_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("dcw-locktest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let held = Lock::acquire(dir.clone());
        let path = dir.join("ports.lock");
        let probe = |arg| Flock::lock(File::open(&path).unwrap(), arg);
        assert!(probe(FlockArg::LockExclusiveNonblock).is_err(), "lock was not exclusive");
        drop(held);
        assert!(probe(FlockArg::LockExclusiveNonblock).is_ok(), "lock not released on drop");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
