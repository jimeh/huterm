//! The versioned handover manifest both approaches carry.

use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub approach: String,
    /// Stable across handovers: the identity clients reconnect to.
    pub server_id: String,
    pub source_build: String,
    pub generation: u64,
    pub run_dir: String,
    pub listener_fd: i32,
    /// Approach A: the copied executable of the source build, for rollback.
    pub rollback_exe: Option<String>,
    pub next_id: u32,
    pub terminals: Vec<TermState>,
    pub handover: Timing,
    pub history: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermState {
    pub id: u32,
    pub pid: i32,
    pub shim_pid: Option<i32>,
    pub master_fd: i32,
    pub status_fd: Option<i32>,
    pub rows: u16,
    pub cols: u16,
    pub capture: String,
    pub bytes: u64,
    pub exit: Option<Exit>,
    pub eof: bool,
    pub closed: bool,
    pub last_read_ns: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum Exit {
    Status(i32),
    /// The exit was observed but its status is unknowable, as when a shim
    /// died before its shell.
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timing {
    pub begin_ns: u64,
    pub precheck_ns: Option<u64>,
    /// When the first reader was told to stop: the start of the blackout.
    pub pause_start_ns: u64,
    /// When every reader had stopped.
    pub paused_ns: u64,
    pub manifest_ns: u64,
    pub handoff_ns: u64,
    pub target_build: String,
    pub nudge: String,
    pub source_pid: i32,
}

/// Validation the importing build applies, also used by `check-handover`.
pub fn validate(
    manifest: &Manifest,
    supported: &[u32],
    approach: &str,
) -> Result<(), String> {
    if !supported.contains(&manifest.format) {
        return Err(format!(
            "unsupported handover format {} (this build reads {:?})",
            manifest.format, supported
        ));
    }
    if manifest.approach != approach {
        return Err(format!("manifest is for approach {}", manifest.approach));
    }
    Ok(())
}

/// Parses a manifest the way a build with a different schema would: with
/// `require_field`, a manifest without that field fails like a missing
/// required field.
pub fn parse(
    body: &str,
    require_field: Option<&str>,
) -> Result<Manifest, String> {
    if let Some(field) = require_field {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|e| e.to_string())?;
        if v.get(field).is_none() {
            return Err(format!("missing field `{field}`"));
        }
    }
    serde_json::from_str(body).map_err(|e| e.to_string())
}
