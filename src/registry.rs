// registry.rs — the port registry (~/.dev-captain/ports.json), the single source of truth
// for port ↔ branch ↔ pid, plus a mkdir-based lock (dcw uses mkdir, not flock).
//
// Schema (same shape as dcw's ~/.dev-captain/ports.json):
//   { "<port>": { "branch": "...", "pid": 0, "started_at": <f64> } }

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Registry::new(),
    }
}

pub fn save(reg: &Path, data: &Registry) {
    if let Some(parent) = reg.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(data) {
        if let Ok(mut f) = std::fs::File::create(reg) {
            let _ = f.write_all(json.as_bytes());
        }
    }
}

/// Simple mutex via mkdir (macOS has no flock). Returns whether acquired.
pub struct Lock {
    dir: PathBuf,
    held: bool,
}

impl Lock {
    pub fn acquire(dir: PathBuf) -> Lock {
        for _ in 0..5 {
            if std::fs::create_dir(&dir).is_ok() {
                return Lock { dir, held: true };
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        eprintln!("warning: could not acquire lock, proceeding without");
        Lock { dir, held: false }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if self.held {
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}