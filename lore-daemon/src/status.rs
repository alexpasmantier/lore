use lore_db::fragment::now_unix;
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
/// icon) and the `lore-daemon status` command can observe what the daemon is
/// doing without IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub state: DaemonState,
    pub pid: u32,
    pub updated_at: i64,
}

pub fn status_file() -> PathBuf {
    lore_db::lore_home().join("daemon.status")
}

pub fn write_status(state: DaemonState) {
    let status = DaemonStatus {
        state,
        pid: std::process::id(),
        updated_at: now_unix(),
    };
    if let Ok(json) = serde_json::to_string(&status) {
        let _ = std::fs::write(status_file(), json);
    }
}

/// Write status on behalf of another process (e.g. restoring daemon's Idle state).
pub fn write_status_for_pid(state: DaemonState, pid: u32) {
    let status = DaemonStatus {
        state,
        pid,
        updated_at: now_unix(),
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
