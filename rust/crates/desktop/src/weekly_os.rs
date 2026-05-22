/// Weekly CEO Operating System — proactive kickoff banner.
///
/// On the first app launch of each new week, the frontend shows a "本周启动"
/// card that sends a structured kickoff prompt to the CEO Agent. The agent
/// reads the company knowledge base (already injected into its system prompt),
/// spins up purpose-built specialist agents, and outputs this week's battle plan.
///
/// State is stored as a single integer (Unix week number) in:
///   {data_dir}/opc-desktop/weekly_kickoff
///
/// If the file is missing or holds a smaller week number, the kickoff is shown.
/// Once the user either starts or skips the kickoff, the file is updated so the
/// banner won't reappear until the next calendar week.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn state_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opc-desktop")
        .join("weekly_kickoff")
}

/// Unix week number: seconds / (7 * 24 * 3600). Monotonically increasing,
/// increments every 7 days. Sufficient for "new week since last kickoff" gating.
fn current_week_number() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / (86_400 * 7))
        .unwrap_or(0)
}

/// Returns `true` when the kickoff banner should be shown.
/// First run (no stored state) → show. New week → show. Same week → hide.
pub fn should_show_kickoff() -> bool {
    let current = current_week_number();
    let path = state_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(stored) = text.trim().parse::<u64>() {
            return stored < current;
        }
    }
    true // No prior record: first launch or missing file.
}

/// Mark the kickoff as handled for this week (either started or skipped).
pub fn mark_kickoff_done() -> Result<(), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, current_week_number().to_string())
        .map_err(|e| e.to_string())
}

fn review_state_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opc-desktop")
        .join("weekly_review")
}

/// Returns true when it's Friday or Saturday AND review hasn't been done this week.
///
/// Day-of-week calculation: 1970-01-01 was a Thursday.
/// days % 7: 0=Thu, 1=Fri, 2=Sat, 3=Sun, 4=Mon, 5=Tue, 6=Wed
pub fn should_show_review() -> bool {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let day_of_week = days % 7; // 1=Fri, 2=Sat
    if day_of_week != 1 && day_of_week != 2 {
        return false; // Only on Friday and Saturday
    }
    // Check if review was done this week
    let current = current_week_number();
    let path = review_state_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(stored) = text.trim().parse::<u64>() {
            return stored < current;
        }
    }
    true
}

pub fn mark_review_done() -> Result<(), String> {
    let path = review_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, current_week_number().to_string()).map_err(|e| e.to_string())
}
