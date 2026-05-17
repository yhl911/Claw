//! Decision Anchors — model-pinned facts that persist across the rest of a
//! session so long conversations don't forget early architectural choices.
//!
//! ## Why this exists
//!
//! Claude (and other long-context LLMs) exhibit the "Lost in the Middle"
//! effect: facts established 50+ turns ago get out-shouted by recent context.
//! The model still has them in raw context but its attention has drifted.
//! Anchoring those facts into the *system prompt* of every subsequent turn
//! reverses the priority: anchored decisions stay at full salience.
//!
//! ## Mental model
//!
//! Think of anchors as "this is settled — don't relitigate." Examples:
//!   - "Use PostgreSQL, not MongoDB. Reason: existing ops infra."
//!   - "Brand voice is technical but friendly, never use exclamation marks."
//!   - "API base URL is https://api.example.com (no trailing slash)."
//!
//! ## Storage
//!
//! `<sessions_dir>/<session_id>.anchors.json` — a flat array of objects with
//! `title`, `rationale`, `pinned_at_secs`. JSON not JSONL so the file is
//! easy to hand-edit if a user wants to clean up.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEntry {
    pub title: String,
    pub rationale: String,
    pub pinned_at_secs: u64,
}

/// Resolve the anchor file path for a session id. Mirrors
/// `session_jsonl_path` so the two live side-by-side and get cleared
/// together when the session is deleted.
pub fn anchors_path(session_id: &str) -> PathBuf {
    crate::state::sessions_dir().join(format!("{session_id}.anchors.json"))
}

pub fn load(session_id: &str) -> Vec<AnchorEntry> {
    let path = anchors_path(session_id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn append(session_id: &str, entry: AnchorEntry) -> std::io::Result<()> {
    let path = anchors_path(session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut list = load(session_id);
    list.push(entry);
    let text = serde_json::to_string_pretty(&list)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, text)
}

/// Render the most-recent N anchors as a Markdown block suitable for
/// injection into the system prompt. Returns an empty string when there
/// are no anchors so the caller can simply skip the section.
pub fn snapshot_for_prompt(session_id: &str, max_recent: usize) -> String {
    let list = load(session_id);
    if list.is_empty() {
        return String::new();
    }
    // Latest first, cap at `max_recent`.
    let mut recent: Vec<&AnchorEntry> = list.iter().collect();
    recent.sort_by(|a, b| b.pinned_at_secs.cmp(&a.pinned_at_secs));
    recent.truncate(max_recent);

    let mut out = String::from(
        "\n## 📌 Pinned Decisions (long-term anchors for this session)\n\n\
         These are facts the user (or you) decided earlier in this session \
         and should remain authoritative. If new context conflicts, prefer \
         these unless the user explicitly overrides.\n\n",
    );
    for a in recent {
        out.push_str(&format!("- **{}** — {}\n", a.title.trim(), a.rationale.trim()));
    }
    out
}

/// Remove all anchors for a session (called when the session is wiped).
pub fn clear(session_id: &str) -> std::io::Result<()> {
    let path = anchors_path(session_id);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn snapshot_returns_empty_for_unknown_session() {
        let out = snapshot_for_prompt("does-not-exist", 5);
        assert!(out.is_empty());
    }

    #[test]
    fn entry_serializes_round_trip() {
        let e = AnchorEntry {
            title: "Use Postgres".to_string(),
            rationale: "Existing infra".to_string(),
            pinned_at_secs: now_secs(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: AnchorEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, "Use Postgres");
        assert_eq!(back.rationale, "Existing infra");
    }
}
