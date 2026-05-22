//! Global decision anchors — cross-session persistent memory.
//! Stored at {data_dir}/opc-desktop/global_anchors.json
//! Injected into every session's system prompt automatically.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalAnchorEntry {
    pub title: String,
    pub rationale: String,
    pub created_at_secs: u64,
    /// Session id where this was originally pinned (for traceability).
    pub source_session: Option<String>,
}

fn path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opc-desktop")
        .join("global_anchors.json")
}

pub fn load() -> Vec<GlobalAnchorEntry> {
    let p = path();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn append(entry: GlobalAnchorEntry) -> std::io::Result<()> {
    let p = path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut list = load();
    // Deduplicate by title: replace if already present.
    if let Some(existing) = list.iter_mut().find(|a| a.title == entry.title) {
        *existing = entry;
    } else {
        list.push(entry);
    }
    let text = serde_json::to_string_pretty(&list)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&p, text)
}

pub fn remove(title: &str) -> std::io::Result<()> {
    let p = path();
    let mut list = load();
    list.retain(|a| a.title != title);
    if list.is_empty() {
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&list)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&p, text)
}

/// Render as a Markdown block for system prompt injection.
/// Returns empty string if there are no global anchors.
pub fn snapshot_for_prompt() -> String {
    let list = load();
    if list.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n## Global Decisions (persistent across all sessions)\n\n\
         These decisions were pinned as globally important. They apply to \
         all sessions and should remain authoritative unless explicitly overridden.\n\n",
    );
    for a in &list {
        out.push_str(&format!("- **{}** -- {}\n", a.title.trim(), a.rationale.trim()));
    }
    out
}
