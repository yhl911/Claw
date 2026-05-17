//! Dreaming-inspired memory consolidation pass.
//!
//! Inspired by Anthropic's May 2026 "dreaming" feature for Claude Managed
//! Agents. Reads recent session transcripts + current memory snapshot,
//! asks the active model to consolidate them into structured memory files,
//! and writes the result back to `.claw/memory/*.md`.
//!
//! The dreaming pass is a single one-shot model call (NOT streaming, NOT
//! tool-using) that returns structured JSON describing the new memory state.
//! We re-use the configured `ProviderClient` from settings so dreaming uses
//! the same model as the main conversation.

use api::{
    InputContentBlock, InputMessage, MessageRequest, ProviderClient, ToolChoice,
};
use runtime::memory::{MemoryFile, MemoryStore, TOP_LEVEL_FILES};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::Path;

/// Helper Raw envelope used by `run_agent_profile_dream` for JSON parsing.
/// Hoisted to module level to satisfy `clippy::items_after_statements`.
#[derive(Deserialize)]
struct ProfileRaw {
    content: String,
    #[serde(default)]
    rationale: String,
}

use crate::config::DesktopConfig;

const DREAM_SYSTEM_PROMPT: &str = r#"
你是 Memory Consolidation Agent ("dreaming pass")。你的任务是审视最近的对话转录和现有长期记忆，提炼出值得长期保留的知识，更新记忆文件。

## 你能写的记忆文件（精简、不超过 ~30 行/文件）

- `facts.md` — 关于用户/项目的稳定事实（偏好、技术栈、约束、命名规范）
- `decisions.md` — 关键的产品/技术决策（"为什么用 X 而不是 Y"）
- `patterns.md` — 反复出现的有效工作模式（"做 X 时按 1-2-3 步效果好"）
- `failures.md` — 已知失败模式和教训（"避免 X，原因是 Y"）

## 输出格式（严格 JSON）

```json
{
  "files": {
    "facts.md": "...",
    "decisions.md": "...",
    "patterns.md": "...",
    "failures.md": "..."
  },
  "rationale": "一句话说明本次最重要的变化"
}
```

## 原则

1. **保留 + 增量更新**：不要把现有记忆全部丢弃；只在需要时合并/补充/修正
2. **去重**：相同含义的两条只留一条
3. **解决矛盾**：以新信息覆盖旧信息，但在 rationale 里说明
4. **简洁**：宁缺毋滥，每个 bullet 1 行
5. **若某文件没有更新内容，原样返回**
6. **若某文件应该清空（旧内容已过时），返回空字符串 `""`**

## 输入

下面会给你：
1. 现有 memory 快照
2. 最近会话转录（按时间倒序）

直接以上述 JSON 格式返回，不要包含任何额外文字或代码块标记。
"#;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DreamProposal {
    pub files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DreamResult {
    pub proposal: DreamProposal,
    /// Existing memory before the pass (for diff display in UI).
    pub previous: std::collections::BTreeMap<String, String>,
}

/// Read JSONL files from `dir` (flat layout: `dir/*.jsonl`), newest-first,
/// up to `char_budget` characters. Returns `None` if dir doesn't exist or
/// is empty, `Some(text)` otherwise.
fn read_from_dir(dir: &std::path::Path, char_budget: usize) -> Option<String> {
    if !dir.exists() {
        return None;
    }
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    files.push((modified, p));
                }
            }
        }
    }
    if files.is_empty() {
        return None;
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = String::new();
    for (_, path) in files {
        if out.len() >= char_budget {
            break;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let remaining = char_budget.saturating_sub(out.len());
            let _ = write!(
                out,
                "\n=== Session: {} ===\n",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
            );
            if content.len() > remaining {
                out.push_str(&content[..remaining]);
                out.push_str("\n[...truncated...]\n");
            } else {
                out.push_str(&content);
            }
        }
    }
    Some(out)
}

/// Read recent session transcripts from `.claw/sessions/<fingerprint>/*.jsonl`
/// (CLI workspace layout). Returns empty string when nothing is found.
fn read_recent_sessions(workspace: &Path, char_budget: usize) -> String {
    let sessions_root = workspace.join(".claw").join("sessions");
    if !sessions_root.exists() {
        return String::new();
    }
    // CLI layout has a fingerprint sub-directory level.
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for fp_dir in std::fs::read_dir(&sessions_root).into_iter().flatten().flatten() {
        let path = fp_dir.path();
        if !path.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        files.push((modified, p));
                    }
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = String::new();
    for (_, path) in files {
        if out.len() >= char_budget {
            break;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let remaining = char_budget.saturating_sub(out.len());
            let _ = write!(
                out,
                "\n=== Session: {} ===\n",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
            );
            if content.len() > remaining {
                out.push_str(&content[..remaining]);
                out.push_str("\n[...truncated...]\n");
            } else {
                out.push_str(&content);
            }
        }
    }
    out
}

/// Run a one-shot dreaming consolidation pass. Does NOT write to disk —
/// returns the proposal so the UI can show a diff and let the user accept.
///
/// `session_id` is the *current* session that is about to be cleared. Its
/// pinned-decision anchors are injected into the dream prompt so they are
/// explicitly considered for long-term memory — anchors are deleted when
/// the session clears, so this is their only promotion opportunity.
pub fn run_dream_pass(
    workspace: &Path,
    config: &DesktopConfig,
    session_id: Option<&str>,
) -> Result<DreamResult, String> {
    let store = MemoryStore::open(workspace);
    let existing: Vec<MemoryFile> = store.read_all().map_err(|e| e.to_string())?;
    let previous: std::collections::BTreeMap<String, String> = existing
        .iter()
        .map(|f| (f.name.clone(), f.content.clone()))
        .collect();

    // Render existing memory snapshot for the prompt.
    let mut existing_block = String::new();
    if existing.is_empty() {
        existing_block.push_str("(no existing memory yet — produce initial files)\n");
    } else {
        for f in &existing {
            let _ = write!(existing_block, "--- {} ---\n{}\n\n", f.name, f.content);
        }
    }

    // Read anchors pinned during the session that is about to be cleared.
    // These are session-scoped and will be deleted after this call, so we
    // feed them explicitly to the dream pass for promotion to long-term memory.
    let anchors_block = if let Some(sid) = session_id {
        let anchors = crate::anchors::load(sid);
        if anchors.is_empty() {
            String::new()
        } else {
            let mut block = String::from(
                "\n\n## 本次会话锚点 (pinned decisions — session-scoped, will be deleted)\n\
                 这些是本次会话里用户或模型明确标记为「需要永久记住」的决策，\
                 请重点考虑是否应写入 decisions.md 或 facts.md：\n\n",
            );
            for a in &anchors {
                let _ = write!(block, "- **{}**: {}\n", a.title, a.rationale);
            }
            block
        }
    } else {
        String::new()
    };

    // Read recent sessions from the desktop app's own storage path first,
    // falling back to the CLI's .claw/sessions/ path in workspaces.
    let desktop_sessions = crate::state::sessions_dir();
    let transcript = read_from_dir(&desktop_sessions, 24_000)
        .or_else(|| {
            let t = read_recent_sessions(workspace, 24_000);
            if t.is_empty() { None } else { Some(t) }
        })
        .unwrap_or_default();

    if transcript.is_empty() && anchors_block.is_empty() {
        return Err("no session transcripts or anchors found to consolidate".to_string());
    }

    let user_text = format!(
        "## 现有记忆 (existing memory)\n\n{existing_block}{anchors_block}\n\n\
         ## 最近会话转录 (recent transcripts, newest first)\n\n\
         {transcript}\n\n\
         请按系统提示返回 JSON。"
    );

    let request = MessageRequest {
        model: crate::config::normalize_model(&config.model),
        max_tokens: 4096,
        messages: vec![InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text { text: user_text }],
            reasoning_content: None,
        }],
        system: Some(DREAM_SYSTEM_PROMPT.to_string()),
        tools: None,
        tool_choice: None as Option<ToolChoice>,
        stream: false,
        thinking_mode: Some(false), // dreaming should be deterministic
        ..Default::default()
    };

    let provider = ProviderClient::from_model(&request.model).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let response = rt
        .block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(180),
                provider.send_message(&request),
            )
            .await
            .map_err(|_| "Dream pass timed out (180s)".to_string())?
            .map_err(|e| e.to_string())
        })?;

    // Extract assistant text.
    let raw_text = response
        .content
        .iter()
        .filter_map(|b| match b {
            api::OutputContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    if raw_text.trim().is_empty() {
        return Err("Dream pass returned empty response".to_string());
    }

    // Strip ```json fences if present.
    let trimmed = raw_text.trim();
    let json_text = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_start_matches('\n')
            .trim_end_matches("```")
            .trim_end_matches('\n')
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_start_matches('\n')
            .trim_end_matches("```")
            .trim_end_matches('\n')
    } else {
        trimmed
    };

    let proposal: DreamProposal = serde_json::from_str(json_text).map_err(|e| {
        format!(
            "Failed to parse dream proposal as JSON: {e}\nRaw response:\n{raw_text}"
        )
    })?;

    Ok(DreamResult { proposal, previous })
}

/// Read all sub-agent manifests for a given role (e.g. `"opc-engineering"`).
/// Returns up to N most-recent entries.
fn read_agent_manifests(
    role: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let agent_dir = tools::agent_store_dir_pub()?;
    if !agent_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&agent_dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        entries.push((modified, path));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let mut out = Vec::new();
    for (_, path) in entries {
        if out.len() >= limit {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest): Result<serde_json::Value, _> = serde_json::from_str(&text)
        else {
            continue;
        };
        let manifest_role = manifest
            .get("subagentType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if manifest_role == role {
            out.push(manifest);
        }
    }
    Ok(out)
}

const PROFILE_SYSTEM_PROMPT: &str = r#"
你是 OPC 子角色画像维护 agent。基于这个角色最近的所有委派 manifest，提炼并更新画像，存到 `.claw/memory/agent_profiles/{role}.md`。

画像应包含 4 个章节（每个 1-5 行 bullet，宁缺毋滥）：

```markdown
## 擅长场景
- ...

## 已知失败模式
- ...

## 推荐 prompt 模板
- "实现 X 功能。要求：1) ... 2) ..."

## 平均完成时间
- 简单任务约 X 分钟，复杂任务约 Y 分钟
```

## 输出格式（严格 JSON）

```json
{
  "content": "完整的 markdown 文件内容",
  "rationale": "一句话说明本次最重要的变化"
}
```

不要包含代码块标记，直接返回 JSON。
"#;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentProfileProposal {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub rationale: String,
    /// Existing content for diff display.
    #[serde(default)]
    pub previous: String,
}

/// Run a dreaming pass that updates `.claw/memory/agent_profiles/{role}.md`
/// based on recent sub-agent manifest history.
pub fn run_agent_profile_dream(
    workspace: &Path,
    config: &DesktopConfig,
    role: &str,
) -> Result<AgentProfileProposal, String> {
    let manifests = read_agent_manifests(role, 50)?;
    if manifests.is_empty() {
        return Err(format!(
            "no manifest history for role '{role}' yet — delegate some tasks first"
        ));
    }

    let store = MemoryStore::open(workspace);
    let profile_name = format!("agent_profiles/{role}.md");
    let previous = store
        .read_all()
        .ok()
        .and_then(|files| {
            files
                .into_iter()
                .find(|f| f.name == profile_name)
                .map(|f| f.content)
        })
        .unwrap_or_default();

    // Render manifests as a compact transcript.
    let mut manifest_block = String::new();
    for (i, m) in manifests.iter().enumerate() {
        let status = m.get("status").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = m.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let err = m.get("error").and_then(|v| v.as_str()).unwrap_or("");
        let started = m.get("startedAt").and_then(|v| v.as_str()).unwrap_or("?");
        let completed = m.get("completedAt").and_then(|v| v.as_str()).unwrap_or("?");
        let _ = write!(
            manifest_block,
            "{}. [{status}] {desc}\n   started: {started}, completed: {completed}\n",
            i + 1
        );
        if !err.is_empty() {
            let _ = writeln!(manifest_block, "   error: {err}");
        }
    }

    let user_text = format!(
        "## 角色\n{role}\n\n## 现有画像\n{}\n\n## 最近 manifest 历史 ({} 条, 最新在前)\n{}\n\n请按系统提示返回 JSON。",
        if previous.is_empty() {
            "(还没有画像)".to_string()
        } else {
            previous.clone()
        },
        manifests.len(),
        manifest_block
    );

    let request = MessageRequest {
        model: crate::config::normalize_model(&config.model),
        max_tokens: 2048,
        messages: vec![InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text { text: user_text }],
            reasoning_content: None,
        }],
        system: Some(PROFILE_SYSTEM_PROMPT.to_string()),
        tools: None,
        tool_choice: None as Option<ToolChoice>,
        stream: false,
        thinking_mode: Some(false),
        ..Default::default()
    };

    let provider = ProviderClient::from_model(&request.model).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let response = rt.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(120),
            provider.send_message(&request),
        )
        .await
        .map_err(|_| "Profile dream timed out (120s)".to_string())?
        .map_err(|e| e.to_string())
    })?;

    let raw_text = response
        .content
        .iter()
        .filter_map(|b| match b {
            api::OutputContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let trimmed = raw_text.trim();
    let json_text = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_start_matches('\n')
            .trim_end_matches("```")
            .trim_end_matches('\n')
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_start_matches('\n')
            .trim_end_matches("```")
            .trim_end_matches('\n')
    } else {
        trimmed
    };

    let raw: ProfileRaw = serde_json::from_str(json_text)
        .map_err(|e| format!("parse profile JSON: {e}\nraw:\n{raw_text}"))?;

    Ok(AgentProfileProposal {
        role: role.to_string(),
        content: raw.content,
        rationale: raw.rationale,
        previous,
    })
}

/// Apply an agent profile proposal directly.
pub fn apply_agent_profile(
    workspace: &Path,
    proposal: &AgentProfileProposal,
) -> Result<(), String> {
    if proposal.role.contains('/') || proposal.role.contains("..") {
        return Err(format!("invalid role: {}", proposal.role));
    }
    let store = MemoryStore::open(workspace);
    store
        .write(
            &format!("agent_profiles/{}.md", proposal.role),
            &proposal.content,
        )
        .map_err(|e| e.to_string())
}

/// Apply a (possibly user-edited) dream proposal to disk. Returns the list
/// of files actually changed.
pub fn apply_dream_proposal(
    workspace: &Path,
    proposal: &DreamProposal,
) -> Result<Vec<String>, String> {
    let store = MemoryStore::open(workspace);
    let mut changed = Vec::new();
    for (name, content) in &proposal.files {
        // Sanity-check the name to prevent path traversal.
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
            return Err(format!("invalid memory file name: {name}"));
        }
        // Only allow conventional names + agent_profiles/*.md
        let is_md = std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        let valid = TOP_LEVEL_FILES.contains(&name.as_str())
            || (name.starts_with("agent_profiles/") && is_md);
        if !valid {
            return Err(format!(
                "memory file name not in allowed list: {name}"
            ));
        }
        let trimmed = content.trim();
        if trimmed.is_empty() {
            // Empty content means delete the file.
            let target = store.base_dir().join(name);
            if target.exists() {
                std::fs::remove_file(&target).map_err(|e| e.to_string())?;
                changed.push(name.clone());
            }
        } else {
            store.write(name, content).map_err(|e| e.to_string())?;
            changed.push(name.clone());
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn apply_proposal_writes_files() {
        let dir = tempdir().unwrap();
        let mut files = std::collections::BTreeMap::new();
        files.insert("facts.md".to_string(), "User likes Rust.\n".to_string());
        files.insert(
            "agent_profiles/opc-engineering.md".to_string(),
            "Strong on refactors.\n".to_string(),
        );
        let proposal = DreamProposal {
            files,
            rationale: "test".to_string(),
        };
        let changed = apply_dream_proposal(dir.path(), &proposal).unwrap();
        assert_eq!(changed.len(), 2);

        let store = MemoryStore::open(dir.path());
        let loaded = store.read_all().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn apply_rejects_path_traversal() {
        let dir = tempdir().unwrap();
        let mut files = std::collections::BTreeMap::new();
        files.insert("../etc/passwd".to_string(), "evil".to_string());
        let proposal = DreamProposal {
            files,
            rationale: String::new(),
        };
        let err = apply_dream_proposal(dir.path(), &proposal).unwrap_err();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn apply_rejects_unknown_file_name() {
        let dir = tempdir().unwrap();
        let mut files = std::collections::BTreeMap::new();
        files.insert("custom.md".to_string(), "hi".to_string());
        let proposal = DreamProposal {
            files,
            rationale: String::new(),
        };
        let err = apply_dream_proposal(dir.path(), &proposal).unwrap_err();
        assert!(err.contains("not in allowed list"));
    }

    #[test]
    fn empty_content_deletes_file() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::open(dir.path());
        store.write("facts.md", "stale").unwrap();

        let mut files = std::collections::BTreeMap::new();
        files.insert("facts.md".to_string(), String::new());
        let proposal = DreamProposal {
            files,
            rationale: String::new(),
        };
        apply_dream_proposal(dir.path(), &proposal).unwrap();
        assert!(store.read_all().unwrap().is_empty());
    }
}
