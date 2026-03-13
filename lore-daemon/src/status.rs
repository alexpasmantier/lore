use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The current activity state of the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonState {
    Idle,
    Ingesting,
    Consolidating,
}

/// Written to `~/.lore/daemon.status` so that external tools (e.g. the tray
/// icon) can observe what the daemon is doing without IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub state: DaemonState,
    pub pid: u32,
    pub updated_at: i64,
}

pub fn status_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".lore").join("daemon.status")
}

pub fn write_status(state: DaemonState) {
    let status = DaemonStatus {
        state,
        pid: std::process::id(),
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    };
    if let Ok(json) = serde_json::to_string(&status) {
        let _ = std::fs::write(status_file(), json);
    }
}

pub fn read_status() -> Option<DaemonStatus> {
    let content = std::fs::read_to_string(status_file()).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn clear_status() {
    let _ = std::fs::remove_file(status_file());
}
