//! Long-running task infrastructure for OPC desktop.
//!
//! "Long task" = a single user request that the runtime is allowed to
//! work on for hours or days, with relaxed limits, automatic retry on
//! transient errors, and on-disk checkpointing so progress survives
//! crashes / restarts.
//!
//! Each task lives under
//! `~/Library/Application Support/opc-desktop/long_tasks/<task-id>/`:
//! ```text
//! spec.json       — immutable task definition (goal, model, limits)
//! state.json      — mutable runtime state (status, iteration, last heartbeat)
//! messages.jsonl  — full conversation history (the runtime's session jsonl)
//! progress.log    — line-per-event progress trail (optional, for debugging)
//! output.md       — final assistant text on success
//! ```
//!
//! At app startup the desktop scans this directory and surfaces any task
//! with `status == "Running"` and a stale heartbeat as "interrupted",
//! letting the user resume or dismiss.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default soft cap on total iterations — generous to allow truly long
/// autonomous loops. The CEO/sub-agent caps still apply per-turn; this
/// covers the *whole* task across all turns/iterations.
pub const DEFAULT_MAX_ITERATIONS: usize = 2000;

/// Heartbeat write cadence. Every N seconds (or every N iterations)
/// we update `state.json` so a crash detector can tell the worker
/// is still alive.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// If a Running task hasn't updated its heartbeat in this long, we
/// declare it interrupted (process likely crashed or app force-quit).
pub const STALE_HEARTBEAT_SECS: u64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
    /// Detected at startup: was Running but heartbeat is stale.
    Interrupted,
}

impl TaskStatus {
    /// True for end states (Done / Failed / Cancelled / Interrupted) —
    /// useful to gate UI affordances like "delete" vs "cancel".
    #[must_use]
    #[allow(dead_code)]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub task_id: String,
    pub goal: String,
    pub model: String,
    pub created_at: u64,
    /// Hard cap on total iterations across the whole task. None = unlimited.
    pub max_total_iterations: Option<usize>,
    /// Wall-clock deadline (Unix secs). None = no deadline.
    pub deadline: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub task_id: String,
    pub status: TaskStatus,
    pub current_iteration: usize,
    /// Total tokens consumed across all turns of this task.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    pub started_at: Option<u64>,
    pub last_heartbeat: u64,
    pub completed_at: Option<u64>,
    pub last_error: Option<String>,
    /// How many transient failures we've retried during this task.
    #[serde(default)]
    pub retry_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub spec: TaskSpec,
    pub state: TaskState,
}

pub fn long_tasks_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("opc-desktop").join("long_tasks")
}

pub fn task_dir(task_id: &str) -> PathBuf {
    long_tasks_dir().join(task_id)
}

pub fn spec_path(task_id: &str) -> PathBuf {
    task_dir(task_id).join("spec.json")
}

pub fn state_path(task_id: &str) -> PathBuf {
    task_dir(task_id).join("state.json")
}

pub fn messages_path(task_id: &str) -> PathBuf {
    task_dir(task_id).join("messages.jsonl")
}

pub fn output_path(task_id: &str) -> PathBuf {
    task_dir(task_id).join("output.md")
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn new_task_id() -> String {
    // Include sub-second precision (nanos) so rapidly-created tasks
    // (tests, batch UI clicks) don't collide.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("lt-{}-{:06}", now.as_secs(), now.subsec_micros())
}

/// Atomic write — write to `<path>.tmp` then rename. Stops half-written
/// state files from confusing the recovery logic if we crash mid-write.
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

pub fn save_spec(spec: &TaskSpec) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(spec).map_err(std::io::Error::other)?;
    write_atomic(&spec_path(&spec.task_id), &json)
}

pub fn save_state(state: &TaskState) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    write_atomic(&state_path(&state.task_id), &json)
}

pub fn load_spec(task_id: &str) -> std::io::Result<TaskSpec> {
    let text = fs::read_to_string(spec_path(task_id))?;
    serde_json::from_str(&text).map_err(std::io::Error::other)
}

pub fn load_state(task_id: &str) -> std::io::Result<TaskState> {
    let text = fs::read_to_string(state_path(task_id))?;
    serde_json::from_str(&text).map_err(std::io::Error::other)
}

/// List every task on disk, newest first by `spec.created_at`.
pub fn list_all() -> Vec<TaskInfo> {
    let dir = long_tasks_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut tasks = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let id = match p.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let Ok(spec) = load_spec(&id) else { continue };
        let Ok(state) = load_state(&id) else { continue };
        tasks.push(TaskInfo { spec, state });
    }
    tasks.sort_by(|a, b| b.spec.created_at.cmp(&a.spec.created_at));
    tasks
}

/// Walk all tasks. For any that claim `Running` but have a stale
/// heartbeat (likely from a crashed worker), flip to `Interrupted` so
/// the UI can ask the user what to do.
///
/// Run this at app startup and (optionally) periodically.
pub fn reap_interrupted() -> std::io::Result<usize> {
    let now = now_secs();
    let mut count = 0;
    for task in list_all() {
        if task.state.status == TaskStatus::Running
            && now.saturating_sub(task.state.last_heartbeat) > STALE_HEARTBEAT_SECS
        {
            let mut updated = task.state.clone();
            updated.status = TaskStatus::Interrupted;
            updated.last_error = Some(format!(
                "Heartbeat stale by {}s — worker likely crashed or app was force-quit",
                now.saturating_sub(task.state.last_heartbeat)
            ));
            save_state(&updated)?;
            count += 1;
        }
    }
    Ok(count)
}

pub fn create_task(goal: &str, model: &str) -> std::io::Result<TaskSpec> {
    let id = new_task_id();
    let spec = TaskSpec {
        task_id: id.clone(),
        goal: goal.to_string(),
        model: model.to_string(),
        created_at: now_secs(),
        max_total_iterations: Some(DEFAULT_MAX_ITERATIONS),
        deadline: None,
    };
    fs::create_dir_all(task_dir(&id))?;
    save_spec(&spec)?;
    let state = TaskState {
        task_id: id,
        status: TaskStatus::Pending,
        current_iteration: 0,
        input_tokens: 0,
        output_tokens: 0,
        started_at: None,
        last_heartbeat: spec.created_at,
        completed_at: None,
        last_error: None,
        retry_count: 0,
    };
    save_state(&state)?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// `with_temp_data_dir` mutates global env vars (HOME, XDG_DATA_HOME) so
    /// `dirs::data_dir()` resolves into our scratch space. Tests that touch
    /// the filesystem must hold this mutex to avoid clobbering each other
    /// when cargo runs them in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_data_dir<F: FnOnce()>(f: F) -> TempDir {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = TempDir::new().unwrap();
        std::env::set_var("XDG_DATA_HOME", dir.path());
        std::env::set_var("HOME", dir.path());
        f();
        dir
    }

    #[test]
    fn create_task_persists_spec_and_state() {
        let _g = with_temp_data_dir(|| {
            let spec = create_task("write a fizzbuzz", "claude-sonnet-4").unwrap();
            assert!(!spec.task_id.is_empty());
            assert_eq!(spec.goal, "write a fizzbuzz");
            let loaded_spec = load_spec(&spec.task_id).unwrap();
            assert_eq!(loaded_spec.task_id, spec.task_id);
            let state = load_state(&spec.task_id).unwrap();
            assert_eq!(state.status, TaskStatus::Pending);
        });
    }

    #[test]
    fn task_status_terminal_classification() {
        assert!(TaskStatus::Done.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
        assert!(TaskStatus::Interrupted.is_terminal());
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
    }

    #[test]
    fn list_all_returns_newest_first() {
        let _g = with_temp_data_dir(|| {
            let s1 = create_task("first", "m").unwrap();
            // Bump created_at deterministically so test isn't subject to
            // sub-second timestamp collisions.
            let mut spec1 = load_spec(&s1.task_id).unwrap();
            spec1.created_at = 1000;
            save_spec(&spec1).unwrap();
            let s2 = create_task("second", "m").unwrap();
            let mut spec2 = load_spec(&s2.task_id).unwrap();
            spec2.created_at = 2000;
            save_spec(&spec2).unwrap();
            let all = list_all();
            assert_eq!(all.len(), 2);
            assert_eq!(all[0].spec.task_id, s2.task_id);
            assert_eq!(all[1].spec.task_id, s1.task_id);
        });
    }

    #[test]
    fn reap_interrupted_flips_stale_running_tasks() {
        let _g = with_temp_data_dir(|| {
            let spec = create_task("stuck", "m").unwrap();
            let mut state = load_state(&spec.task_id).unwrap();
            state.status = TaskStatus::Running;
            state.last_heartbeat = now_secs().saturating_sub(STALE_HEARTBEAT_SECS + 100);
            save_state(&state).unwrap();

            let count = reap_interrupted().unwrap();
            assert!(count >= 1);

            let after = load_state(&spec.task_id).unwrap();
            assert_eq!(after.status, TaskStatus::Interrupted);
        });
    }
}
