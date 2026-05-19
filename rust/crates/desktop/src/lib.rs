// Tauri commands legitimately need owned types (`String`, owned structs) for
// IPC deserialization, so `needless_pass_by_value` doesn't apply. Underscore
// binding is also a Tauri convention (the `state` arg is required for trait
// dispatch even when unused).
#![allow(
    clippy::needless_pass_by_value,
    clippy::used_underscore_binding,
    clippy::doc_markdown,
    clippy::too_many_lines,
    // Markdown rendering builds a string with many concatenations — using
    // write! would force std::fmt::Write imports everywhere and isn't
    // measurably faster for human-scale documents.
    clippy::format_push_string,
    // Many helpers are now pub for the daemon crate. Clippy starts
    // flagging them for #[must_use] candidate-ness, but most are filesystem
    // path builders or constructors whose return values are never
    // accidentally discarded in practice.
    clippy::must_use_candidate
)]

mod anchors;
mod api_client;
mod compaction;
mod company;
mod context_window;
mod daemon_client;
mod dream;
mod loop_detector;
mod mcp;
mod permission;
mod state;
mod token_stats;
mod tool_executor;
mod web_search;

// These modules are re-exposed as `pub mod` so the `opc-daemon` binary
// crate (which depends on `opc_desktop_lib`) can call into them without
// duplicating the runner / task storage logic.
pub mod config;
pub mod event_sink;
pub mod hooks;
pub mod long_runner;
pub mod long_task;
pub mod skills;
pub mod vault;

use config::{apply_config_to_env, load_config, save_config, DesktopConfig};
use state::{AppState, DesktopState, ImagePayload, OpcAgentInfo, TurnResult, WorkerMsg};
use tauri::{Emitter, State};

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    message: String,
    #[allow(clippy::default_trait_access)]
    images: Option<Vec<ImagePayload>>,
) -> Result<TurnResult, String> {
    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
    state
        .tx
        .send(WorkerMsg::SendMessage {
            text: message,
            images: images.unwrap_or_default(),
            responder: resp_tx,
        })
        .map_err(|_| "Worker is not running. Please check your API key settings and restart.".to_string())?;
    // Use spawn_blocking so the sync `recv()` does not block a Tauri/Tokio
    // executor thread. Without this, while `run_turn` is in flight (often
    // 30s-3min) the executor's worker thread is stuck and other async
    // commands queue up — making the UI appear frozen.
    tokio::task::spawn_blocking(move || resp_rx.recv())
        .await
        .map_err(|e| format!("spawn_blocking join: {e}"))?
        .map_err(|e| format!("Worker channel closed: {e}"))
        .and_then(|r| r)
}

/// Read agent manifests directly from disk — does NOT go through the worker
/// channel. This is critical: while the worker is blocked in `run_turn`,
/// any command that talks to the channel would queue up and never return,
/// freezing the UI's poll loop and depleting Tauri's command thread pool.
#[tauri::command]
fn list_opc_agents() -> Vec<OpcAgentInfo> {
    read_opc_agents()
}

#[tauri::command]
async fn clear_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let cfg = load_config();
    let do_auto_dream = cfg.auto_dream;
    let mode = cfg.auto_dream_mode.clone();

    let sess_id_for_hook = state::read_or_init_current_session_id();
    hooks::dispatch(
        &cfg.hooks,
        &hooks::HookContext::new("before_clear_session", sess_id_for_hook.clone()),
    );

    // Run dreaming BEFORE the session is wiped so transcripts are still on disk.
    if do_auto_dream {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let cfg_clone = cfg.clone();
        let sess_id_for_dream = sess_id_for_hook.clone();
        let dream_handle = tokio::task::spawn_blocking(move || {
            apply_config_to_env(&cfg_clone);
            dream::run_dream_pass(&cwd, &cfg_clone, Some(&sess_id_for_dream))
        })
        .await
        .map_err(|e| e.to_string())?;

        match dream_handle {
            Ok(result) => {
                if mode == "apply" {
                    let cwd2 = std::env::current_dir().map_err(|e| e.to_string())?;
                    if let Err(e) = dream::apply_dream_proposal(&cwd2, &result.proposal) {
                        eprintln!("[clear_session] auto-dream apply failed: {e}");
                    } else {
                        let _ = app.emit("dream-applied", &result.proposal.rationale);
                    }
                } else {
                    // review mode — push proposal to UI; user decides
                    let _ = app.emit("dream-pending", &result);
                }
            }
            Err(e) => {
                eprintln!("[clear_session] auto-dream failed: {e}");
                // Don't fail the whole clear_session — proceed with reset
            }
        }
    }

    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
    state
        .tx
        .send(WorkerMsg::ClearSession { responder: resp_tx })
        .map_err(|_| "Worker not running".to_string())?;
    let result = tokio::task::spawn_blocking(move || resp_rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
        .and_then(|r| r);

    // Best-effort: also wipe terminal-state agent manifests so the right
    // panel doesn't keep accumulating cruft across sessions. Running agents
    // (those still in flight in the worker) are left alone — their manifests
    // will appear again when they finish.
    let removed = clear_agents(vec!["completed".to_string(), "failed".to_string()])
        .unwrap_or_else(|e| {
            eprintln!("[clear_session] agent cleanup skipped: {e}");
            0
        });
    if removed > 0 {
        eprintln!("[clear_session] removed {removed} terminal agent manifest(s)");
    }

    hooks::dispatch(
        &cfg.hooks,
        &hooks::HookContext::new("after_clear_session", sess_id_for_hook),
    );

    result
}

/// List the pinned-decision anchors for the current session, most recent
/// first. Used by the right-side anchors panel.
#[tauri::command]
fn list_anchors() -> Vec<anchors::AnchorEntry> {
    let id = state::read_or_init_current_session_id();
    let mut list = anchors::load(&id);
    list.sort_by(|a, b| b.pinned_at_secs.cmp(&a.pinned_at_secs));
    list
}

/// Remove a single anchor (matched by exact title — anchors aren't
/// numbered, so the title is the practical handle).
#[tauri::command]
fn remove_anchor(title: String) -> Result<(), String> {
    let id = state::read_or_init_current_session_id();
    let mut list = anchors::load(&id);
    list.retain(|a| a.title != title);
    let path = anchors::anchors_path(&id);
    if list.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Trigger a one-shot compaction of the current session. Summarizes the
/// older half of messages into a single synthetic exchange. Returns the
/// per-session report (or `null` if nothing was compacted because the
/// session was too short or no safe cut-point existed).
#[tauri::command]
async fn compact_session_now(
    state: State<'_, AppState>,
) -> Result<Option<compaction::CompactionReport>, String> {
    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
    state
        .tx
        .send(WorkerMsg::CompactSession { responder: resp_tx })
        .map_err(|_| "Worker not running".to_string())?;
    tokio::task::spawn_blocking(move || resp_rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
        .and_then(|r| r)
}

#[tauri::command]
fn get_settings() -> DesktopConfig {
    load_config()
}

#[tauri::command]
fn save_settings(
    state: State<'_, AppState>,
    config: DesktopConfig,
) -> Result<(), String> {
    save_config(&config)?;
    // Notify worker to reinitialize with new config
    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
    let _ = state.tx.send(WorkerMsg::Reinitialize {
        config,
        responder: resp_tx,
    });
    resp_rx.recv().unwrap_or(Ok(()))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentDetail {
    pub manifest: serde_json::Value,
    pub output: Option<String>,
}

/// One historical message rendered for the UI on app restart so the user
/// sees their previous conversation. Kept minimal — UI uses this only to
/// re-mount the bubble list, not to drive any logic.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoredMessage {
    pub id: String,
    pub role: String, // "user" | "assistant"
    pub text: String,
}

/// Render the current persisted session as a single Markdown document.
/// Includes the user/assistant transcript and, when sub-agent outputs are
/// referenced via tool_use blocks, inlines those final outputs too.
///
/// Returned `(filename, content)` — the UI passes both to the dialog
/// plugin's `save` API so the user can pick a destination.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportedSession {
    pub filename: String,
    pub content: String,
}

#[tauri::command]
fn export_session() -> Result<ExportedSession, String> {
    use runtime::{ContentBlock, MessageRole, Session};

    let id = state::read_or_init_current_session_id();
    let path = state::session_jsonl_path(&id);
    if !path.exists() {
        return Err("No session to export — start a conversation first.".to_string());
    }
    let session = Session::load_from_path(&path).map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str("# OPC Conversation Export\n\n");
    out.push_str(&format!("- session_id: `{}`\n", session.session_id));
    out.push_str(&format!(
        "- exported_at: {}\n\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    out.push_str("---\n\n");

    // Try to map agent_id → role label for nicer rendering of tool_use blocks
    // that referenced sub-agents.
    let agent_role_lookup = collect_agent_role_labels();

    for msg in &session.messages {
        match msg.role {
            MessageRole::User => {
                out.push_str("## 👤 用户\n\n");
                for block in &msg.blocks {
                    if let ContentBlock::Text { text } = block {
                        out.push_str(text);
                        out.push_str("\n\n");
                    }
                }
            }
            MessageRole::Assistant => {
                out.push_str("## 🤖 CEO\n\n");
                let mut wrote_text = false;
                for block in &msg.blocks {
                    match block {
                        ContentBlock::Text { text } if !text.trim().is_empty() => {
                            out.push_str(text);
                            out.push_str("\n\n");
                            wrote_text = true;
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            // Render tool calls as compact summary blocks.
                            if name == "Agent" {
                                let parsed: serde_json::Value =
                                    serde_json::from_str(input).unwrap_or(serde_json::Value::Null);
                                let role = parsed
                                    .get("subagent_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let role_label = agent_role_lookup
                                    .get(role)
                                    .map_or(role, String::as_str);
                                let desc = parsed
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                out.push_str(&format!(
                                    "> 🚀 委派给 **{role_label}**: {desc}\n\n"
                                ));
                            } else {
                                out.push_str(&format!("> 🔧 调用 `{name}` 工具\n\n"));
                            }
                            wrote_text = true;
                        }
                        _ => {}
                    }
                }
                if !wrote_text {
                    out.push_str("_(no visible output)_\n\n");
                }
            }
            MessageRole::Tool => {
                // For Agent tool results, inline the sub-agent's final output.
                for block in &msg.blocks {
                    if let ContentBlock::ToolResult { output, .. } = block {
                        // Try to detect Agent tool result — has `output` field
                        // with the sub-agent's full text.
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(output) {
                            if let Some(text) =
                                parsed.get("output").and_then(|v| v.as_str())
                            {
                                if !text.trim().is_empty() {
                                    let role = parsed
                                        .get("subagentType")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("sub-agent");
                                    let role_label = agent_role_lookup
                                        .get(role)
                                        .map_or(role, String::as_str);
                                    out.push_str(&format!(
                                        "<details><summary>📄 {role_label} sub-agent 输出</summary>\n\n{text}\n\n</details>\n\n"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            MessageRole::System => {
                // skip system messages from the export
            }
        }
        out.push_str("---\n\n");
    }

    let filename = format!(
        "opc-conversation-{}.md",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    Ok(ExportedSession {
        filename,
        content: out,
    })
}

/// Companion command: write `content` to `path` (the user-picked save
/// destination from the dialog plugin). Refuses anything that doesn't end
/// in `.md` to avoid surprises like overwriting executables.
#[tauri::command]
fn write_export(path: String, content: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    let ext_ok = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    if !ext_ok {
        return Err(format!(
            "refusing to write non-.md file: {path} — append .md to the filename"
        ));
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {e}"))?;
    }
    std::fs::write(&p, content).map_err(|e| format!("write: {e}"))
}

fn ceo_iter_limit_for_display() -> usize {
    std::env::var("CLAWD_CEO_MAX_ITERATIONS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(200)
}

fn collect_agent_role_labels() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    map.insert("opc-product".into(), "产品 (opc-product)".into());
    map.insert("opc-engineering".into(), "工程 (opc-engineering)".into());
    map.insert("opc-finance".into(), "财务 (opc-finance)".into());
    map.insert("opc-marketing".into(), "市场 (opc-marketing)".into());
    map.insert("opc-sales".into(), "销售 (opc-sales)".into());
    map.insert("opc-ops".into(), "运营 (opc-ops)".into());
    map.insert("opc-legal".into(), "法务 (opc-legal)".into());
    map
}

/// Load the persisted desktop session and return user-visible text
/// messages. System messages and tool I/O are filtered out — the UI
/// already renders tool calls live during their turn, and replaying tool
/// machinery on restore would be confusing.
// ── Multi-session commands ───────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    /// Unix seconds — earliest message in the session, or session creation
    /// time if no messages exist yet.
    pub created_at: u64,
    /// File mtime as a recency proxy.
    pub updated_at: u64,
    pub message_count: usize,
}

/// Derive a human-readable title from the first user message in a session
/// jsonl. Truncated at the first newline and ~30 chars. Empty session gets
/// a generic placeholder.
fn derive_session_title(session: &runtime::Session) -> String {
    use runtime::{ContentBlock, MessageRole};
    for msg in &session.messages {
        if !matches!(msg.role, MessageRole::User) {
            continue;
        }
        for block in &msg.blocks {
            if let ContentBlock::Text { text } = block {
                let line = text.lines().next().unwrap_or("").trim();
                if line.is_empty() {
                    continue;
                }
                // Limit to ~30 characters (CJK-aware)
                let truncated: String = line.chars().take(30).collect();
                if truncated.chars().count() < line.chars().count() {
                    return format!("{truncated}…");
                }
                return truncated;
            }
        }
    }
    String::from("(空会话)")
}

/// List every session jsonl in the sessions directory with metadata. Sorted
/// newest-first by `updated_at` so the sidebar shows recent work at top.
#[tauri::command]
fn list_sessions() -> Result<Vec<SessionInfo>, String> {
    let dir = state::sessions_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let updated_at = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        // Skip parse errors — list is best-effort
        let Ok(session) = runtime::Session::load_from_path(&path) else {
            continue;
        };
        let title = derive_session_title(&session);
        let message_count = session.messages.len();
        let created_at = session.created_at_ms / 1000;
        out.push(SessionInfo {
            id,
            title,
            created_at,
            updated_at,
            message_count,
        });
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

#[tauri::command]
fn current_session_id() -> String {
    state::read_or_init_current_session_id()
}

// ── Long-running task commands ──────────────────────────────────────────────

/// Start a new long-running task. Returns the assigned task_id immediately;
/// execution happens on a fresh background thread that survives this
/// command's return. Watch progress via `list_long_tasks` / `get_long_task`
/// (or the `long-task-changed` Tauri event).
#[tauri::command]
fn start_long_task(
    _state: State<'_, AppState>,
    _app: tauri::AppHandle,
    goal: String,
) -> Result<String, String> {
    let trimmed = goal.trim();
    if trimmed.is_empty() {
        return Err("goal must not be empty".to_string());
    }

    // Always delegate to the daemon so the task survives desktop close.
    // The daemon owns the task — we never spawn an in-process runner here.
    daemon_client::ensure_running()?;
    let cfg = load_config();
    let task_id = daemon_client::start_task(trimmed, Some(&cfg.model))?;
    eprintln!("[lib] daemon accepted long task: {task_id}");
    Ok(task_id)
}

/// Resume an Interrupted (or Failed) long task. Reuses the existing
/// session jsonl so the model picks up its prior reasoning. Sends a
/// short continuation nudge instead of re-pushing the original goal.
///
/// Refuses to resume tasks that are already Running or Done.
#[tauri::command]
fn resume_long_task(
    _state: State<'_, AppState>,
    _app: tauri::AppHandle,
    task_id: String,
) -> Result<(), String> {
    use long_task::TaskStatus;
    let info = get_long_task(task_id.clone())?;
    if !matches!(
        info.state.status,
        TaskStatus::Interrupted | TaskStatus::Failed | TaskStatus::Cancelled
    ) {
        return Err(format!(
            "cannot resume task in state {:?} (only Interrupted/Failed/Cancelled can be resumed)",
            info.state.status
        ));
    }
    daemon_client::ensure_running()?;
    daemon_client::resume_task(&task_id)?;
    eprintln!("[lib] daemon accepted resume for: {task_id}");
    Ok(())
}

#[tauri::command]
fn list_long_tasks() -> Vec<long_task::TaskInfo> {
    long_task::list_all()
}

#[tauri::command]
fn get_long_task(task_id: String) -> Result<long_task::TaskInfo, String> {
    let spec = long_task::load_spec(&task_id).map_err(|e| format!("load spec: {e}"))?;
    let state = long_task::load_state(&task_id).map_err(|e| format!("load state: {e}"))?;
    Ok(long_task::TaskInfo { spec, state })
}

/// Read the final output.md (only present after a Done task). Returns
/// empty string if the task hasn't produced output yet.
#[tauri::command]
fn read_long_task_output(task_id: String) -> Result<String, String> {
    let path = long_task::output_path(&task_id);
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Request cancellation of a running long task. The runner checks the
/// flag between SSE events and exits with status=Cancelled.
#[tauri::command]
fn cancel_long_task(_state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    // Delegate to daemon — only the daemon knows which tasks are still
    // running. If the daemon isn't reachable, the task isn't running.
    if !daemon_client::is_running() {
        return Err("daemon not running — task is already stopped".to_string());
    }
    daemon_client::cancel_task(&task_id)
}

/// Permanently delete a terminal long task. Refuses to delete tasks
/// still considered Running on disk.
#[tauri::command]
fn delete_long_task(_state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    if let Ok(info) = long_task::load_state(&task_id) {
        if info.status == long_task::TaskStatus::Running {
            return Err(
                "task is still Running on disk. Cancel it first, then delete.".to_string(),
            );
        }
    }
    let dir = long_task::task_dir(&task_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("remove task dir: {e}"))?;
    }
    Ok(())
}

/// Aggregate token usage across the persisted log. Cheap enough to call
/// every few seconds — log is JSONL with one record per turn.
#[tauri::command]
fn get_token_stats() -> token_stats::TokenStats {
    token_stats::read_stats()
}

/// Switch the worker to an existing session id. Re-saves current_id and
/// rebuilds the runtime so the next `restore_session` shows the new
/// session's history.
#[tauri::command]
async fn switch_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
    state
        .tx
        .send(WorkerMsg::SwitchSession {
            new_id: session_id,
            responder: resp_tx,
        })
        .map_err(|_| "Worker not running".to_string())?;
    tokio::task::spawn_blocking(move || resp_rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
        .and_then(|r| r)
}

/// Create a brand-new session and switch into it.
#[tauri::command]
async fn new_session(state: State<'_, AppState>) -> Result<String, String> {
    let id = state::new_session_id();
    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
    state
        .tx
        .send(WorkerMsg::SwitchSession {
            new_id: id.clone(),
            responder: resp_tx,
        })
        .map_err(|_| "Worker not running".to_string())?;
    tokio::task::spawn_blocking(move || resp_rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
        .and_then(|r| r)?;
    Ok(id)
}

/// Permanently delete a session's jsonl file. Refuses to delete the
/// currently-active session — the user has to switch away first.
#[tauri::command]
fn delete_session(session_id: String) -> Result<(), String> {
    let current = state::read_or_init_current_session_id();
    if session_id == current {
        return Err(
            "Cannot delete the active session. Switch to another one first.".to_string(),
        );
    }
    let path = state::session_jsonl_path(&session_id);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn restore_session() -> Result<Vec<RestoredMessage>, String> {
    use runtime::{ContentBlock, MessageRole, Session};
    let id = state::read_or_init_current_session_id();
    let path = state::session_jsonl_path(&id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let session = Session::load_from_path(&path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (i, msg) in session.messages.iter().enumerate() {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            // System / Tool: skip (the UI doesn't render these as bubbles)
            _ => continue,
        };
        let text: String = msg
            .blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
        if text.is_empty() {
            continue;
        }
        out.push(RestoredMessage {
            id: format!("restore-{i}"),
            role: role.to_string(),
            text,
        });
    }
    Ok(out)
}

/// Read full manifest + output for a specific OPC sub-agent.
#[tauri::command]
fn read_agent_detail(agent_id: String) -> Result<AgentDetail, String> {
    let agent_dir = tools::agent_store_dir_pub()?;
    let manifest_path = agent_dir.join(format!("{agent_id}.json"));
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read manifest: {e}"))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text)
        .map_err(|e| format!("parse manifest: {e}"))?;
    let output_path = manifest
        .get("outputFile")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from);
    let output = output_path.and_then(|p| std::fs::read_to_string(&p).ok());
    Ok(AgentDetail { manifest, output })
}

/// Signal the running turn to abort. The streaming loop checks this flag
/// between SSE events and bails with a clean cancellation error. Has no
/// effect when no turn is in flight (the flag is reset at each turn start).
#[tauri::command]
fn cancel_turn(state: State<'_, AppState>) {
    state
        .cancel_flag
        .store(true, std::sync::atomic::Ordering::SeqCst);
    eprintln!("[lib] cancel_turn requested");
}

/// Dismiss a sub-agent's manifest (purely cosmetic — does not interrupt
/// the worker thread, which Rust cannot safely kill mid-execution; the
/// thread will finish and write a stale manifest if still running).
#[tauri::command]
fn dismiss_agent(agent_id: String) -> Result<(), String> {
    let agent_dir = tools::agent_store_dir_pub()?;
    let manifest_path = agent_dir.join(format!("{agent_id}.json"));
    if manifest_path.exists() {
        std::fs::remove_file(&manifest_path).map_err(|e| format!("remove manifest: {e}"))?;
    }
    Ok(())
}

/// Bulk cleanup: delete every agent in the store whose status matches one
/// of `statuses`. Use `["completed", "failed"]` to "clear terminal", or
/// `["completed", "failed", "running"]` to clear everything (running ones
/// are still left alive in the worker thread — see `dismiss_agent` doc).
/// Returns the number of agents removed.
#[tauri::command]
fn clear_agents(statuses: Vec<String>) -> Result<usize, String> {
    let agent_dir = tools::agent_store_dir_pub()?;
    if !agent_dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(&agent_dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
            continue;
        };
        let status = manifest.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if !statuses.iter().any(|s| s == status) {
            continue;
        }

        // Remove manifest .json
        let _ = std::fs::remove_file(&path);
        // Remove paired output .md if present
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !stem.is_empty() {
            let md_path = path.with_extension("md");
            let _ = std::fs::remove_file(&md_path);
        }
        removed += 1;
    }
    Ok(removed)
}

/// Run a dreaming consolidation pass and return the proposal for UI review.
/// Does NOT write to disk — the user must call `apply_dream` to persist.
#[tauri::command]
async fn run_dream() -> Result<dream::DreamResult, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let cfg = load_config();
    apply_config_to_env(&cfg);
    // Manual dream trigger — not associated with a specific session clear,
    // so no session_id to pass for anchor injection.
    tokio::task::spawn_blocking(move || dream::run_dream_pass(&cwd, &cfg, None))
        .await
        .map_err(|e| e.to_string())?
}

/// Apply a (possibly user-edited) dream proposal to disk.
#[tauri::command]
fn apply_dream(proposal: dream::DreamProposal) -> Result<Vec<String>, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    dream::apply_dream_proposal(&cwd, &proposal)
}

/// Run a dreaming pass that builds an agent profile for one OPC role.
#[tauri::command]
async fn run_agent_profile_dream(
    role: String,
) -> Result<dream::AgentProfileProposal, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let cfg = load_config();
    apply_config_to_env(&cfg);
    tokio::task::spawn_blocking(move || dream::run_agent_profile_dream(&cwd, &cfg, &role))
        .await
        .map_err(|e| e.to_string())?
}

/// Apply an agent profile proposal to disk.
#[tauri::command]
fn apply_agent_profile(proposal: dream::AgentProfileProposal) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    dream::apply_agent_profile(&cwd, &proposal)
}

/// Read a file's text content for use as an inline attachment in the chat.
/// Limited to reasonable text sizes to avoid blowing the prompt.
#[tauri::command]
fn read_attachment(path: String) -> Result<String, String> {
    const MAX_BYTES: u64 = 256 * 1024; // 256 KB cap
    let metadata = std::fs::metadata(&path).map_err(|e| format!("stat: {e}"))?;
    if metadata.len() > MAX_BYTES {
        return Err(format!(
            "file too large ({} bytes); attachments are limited to {} bytes",
            metadata.len(),
            MAX_BYTES
        ));
    }
    std::fs::read_to_string(&path).map_err(|e| format!("read: {e}"))
}

#[tauri::command]
fn get_company_context() -> Result<String, String> {
    Ok(company::read_company_context().unwrap_or_default())
}

#[tauri::command]
fn save_company_context(text: String) -> Result<(), String> {
    company::write_company_context(&text)
}

/// Return the list of available slash commands (drawn from the runtime's
/// command registry). For Phase 3 the desktop only displays them — actual
/// slash command parsing happens in the chat input pipeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SlashCommandInfo {
    pub name: String,
    pub summary: String,
}

#[tauri::command]
fn list_slash_commands() -> Vec<SlashCommandInfo> {
    // Hand-curated set most useful from the desktop. The CLI has many more
    // (in `commands` crate) but desktop UX needs only a curated quick-start.
    vec![
        SlashCommandInfo {
            name: "/clear".to_string(),
            summary: "清空当前会话（如开启 auto-dream 会先固化记忆）".to_string(),
        },
        SlashCommandInfo {
            name: "/dream".to_string(),
            summary: "立刻运行 dreaming consolidation".to_string(),
        },
        SlashCommandInfo {
            name: "/agents".to_string(),
            summary: "在右侧 panel 查看活跃的 OPC sub-agents".to_string(),
        },
        SlashCommandInfo {
            name: "/memory".to_string(),
            summary: "查看当前长期记忆文件".to_string(),
        },
        SlashCommandInfo {
            name: "/settings".to_string(),
            summary: "打开设置".to_string(),
        },
    ]
}

/// Probe the configured MCP servers and return per-server status (tool
/// counts, errors). Drives the "MCP Servers" panel in Settings so users
/// can see at a glance which of their servers are healthy. Note this
/// spawns each MCP process briefly to enumerate tools — it isn't free.
#[tauri::command]
async fn get_mcp_status() -> Result<mcp::McpRuntimeStatus, String> {
    tokio::task::spawn_blocking(|| {
        let cfg = load_config();
        Ok::<_, String>(mcp::probe_status(&cfg.mcp_servers))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// Per-model + 14-day daily breakdown of cost for the Costs panel.
/// Window seconds default to the last 30 days when 0 is passed.
#[tauri::command]
fn get_cost_breakdown(window_secs: Option<u64>) -> token_stats::CostBreakdown {
    let window = window_secs.unwrap_or(0);
    let effective = if window == 0 { 30 * 86_400 } else { window };
    token_stats::cost_breakdown(effective)
}

/// Per-model quality breakdown — avg iterations per turn and tool error
/// rate. Feeds the Quality tab of the CostsPanel so users can spot which
/// model "wanders" or fails more on their workload.
#[tauri::command]
fn get_quality_breakdown(window_secs: Option<u64>) -> token_stats::QualityBreakdown {
    let window = window_secs.unwrap_or(0);
    let effective = if window == 0 { 30 * 86_400 } else { window };
    token_stats::quality_breakdown(effective)
}

// ── Skills management ─────────────────────────────────────────────────────────
//
// Skills live in `~/Library/Application Support/opc-desktop/skills/<name>/SKILL.md`
// and are loaded on demand by the existing `Skill` tool (the runtime
// discovers them via `CLAW_CONFIG_HOME`, set by `apply_config_to_env`).

#[tauri::command]
fn list_skills() -> Vec<skills::SkillInfo> {
    skills::list_skills()
}

#[tauri::command]
fn create_skill(
    name: String,
    description: String,
    body: String,
) -> Result<skills::SkillInfo, String> {
    skills::create_skill(&name, &description, &body)
}

#[tauri::command]
fn delete_skill(name: String) -> Result<(), String> {
    skills::delete_skill(&name)
}

#[tauri::command]
fn toggle_skill(name: String, enabled: bool) -> Result<(), String> {
    skills::toggle_skill(&name, enabled)
}

#[tauri::command]
fn read_skill(name: String) -> Result<String, String> {
    skills::read_skill(&name)
}

#[tauri::command]
async fn list_remote_skills(repo: Option<String>) -> Result<Vec<skills::RemoteSkill>, String> {
    skills::list_remote_skills(repo).await
}

#[tauri::command]
async fn import_remote_skill(
    repo: Option<String>,
    path: String,
) -> Result<skills::SkillInfo, String> {
    skills::import_remote_skill(repo, &path).await
}

#[tauri::command]
fn import_local_skill(path: String) -> Result<skills::SkillInfo, String> {
    skills::import_local_skill(std::path::Path::new(&path))
}

/// List current memory files for read-only display.
#[tauri::command]
fn list_memory_files() -> Result<Vec<MemoryFileInfo>, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let store = runtime::memory::MemoryStore::open(&cwd);
    let files = store.read_all().map_err(|e| e.to_string())?;
    Ok(files
        .into_iter()
        .map(|f| MemoryFileInfo {
            name: f.name,
            content: f.content,
        })
        .collect())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryFileInfo {
    pub name: String,
    pub content: String,
}

// ── Worker thread ─────────────────────────────────────────────────────────────

fn extract_text(summary: &runtime::TurnSummary) -> String {
    use runtime::ContentBlock;
    summary
        .assistant_messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|block| {
            if let ContentBlock::Text { text } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn read_opc_agents() -> Vec<OpcAgentInfo> {
    let Ok(agent_dir) = tools::agent_store_dir_pub() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&agent_dir) else {
        return Vec::new();
    };

    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        // Manifests serialize keys as camelCase (`subagentType`); accept the
        // snake_case form too for forward compatibility.
        let subagent_type = manifest
            .get("subagentType")
            .or_else(|| manifest.get("subagent_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !subagent_type.starts_with("opc-") {
            continue;
        }
        // `createdAt` in the manifest is a stringified Unix-seconds (see
        // `iso8601_now` in tools/lib.rs which is misleadingly named — it
        // actually returns secs as a string). Parse for grouping/sorting.
        let created_at_secs = manifest
            .get("createdAt")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        agents.push(OpcAgentInfo {
            id: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
            subagent_type,
            status: manifest
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            description: manifest
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            created_at_secs,
        });
    }
    // Newest first so the panel shows the most recent turn at the top.
    agents.sort_by(|a, b| b.created_at_secs.cmp(&a.created_at_secs));
    agents
}

fn worker_loop(
    rx: std::sync::mpsc::Receiver<WorkerMsg>,
    cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    app_handle: tauri::AppHandle,
) {
    let mut cfg = load_config();
    eprintln!("[worker] starting. model={:?} api_key_set={} base_url={:?}",
        cfg.model, !cfg.api_key.is_empty(), cfg.base_url);
    apply_config_to_env(&cfg);

    // Determine which session to load on startup. On first launch this
    // generates a new id and persists it.
    let mut current_session_id = state::read_or_init_current_session_id();
    eprintln!("[worker] current session id: {current_session_id}");

    let mut desktop_state: Option<DesktopState> = match DesktopState::build(&cfg, cancel_flag.clone(), event_sink::tauri_sink(app_handle.clone()), current_session_id.clone()) {
        Ok(s) => {
            eprintln!("[worker] DesktopState built OK");
            Some(s)
        }
        Err(e) => {
            eprintln!("[worker] init error: {e}");
            None
        }
    };

    while let Ok(msg) = rx.recv() {
        match msg {
            WorkerMsg::SendMessage { text, images, responder } => {
                let preview: String = text.chars().take(80).collect();
                eprintln!("[worker] SendMessage received: {preview:?} ({} images)", images.len());
                // Enforce budget caps before we spend money. The check is
                // a single linear pass over the (tiny) token log so the
                // cost of checking is negligible compared to the API call
                // we're about to skip.
                if let Err(reason) = token_stats::check_budget(
                    cfg.budget_daily_usd,
                    cfg.budget_monthly_usd,
                ) {
                    eprintln!("[worker] budget blocked: {reason}");
                    let _ = responder.send(Err(reason));
                    continue;
                }
                let result = if desktop_state.is_some() {
                    eprintln!("[worker] calling run_turn...");
                    // Tell the UI a new streaming turn is starting so it can
                    // mount an empty in-progress assistant bubble.
                    let _ = app_handle.emit("turn-start", ());

                    // Wrap run_turn in catch_unwind so a panic anywhere in the
                    // runtime / api_client / tools layer doesn't kill the
                    // worker thread (which would close the channel and brick
                    // the app until restart). On panic we rebuild the state
                    // so the next message starts cleanly.
                    let state_ref = desktop_state.as_mut().unwrap();
                    // Reset the loop detector before each turn so that calls
                    // from *previous* turns don't cause false-positive loop
                    // detection in the current turn.
                    state_ref.runtime.tool_executor_mut().reset_for_new_turn();
                    let panic_result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| {
                            let mut approver = permission::OpcApprover;
                            let prompter: Option<&mut dyn runtime::PermissionPrompter> =
                                Some(&mut approver);
                            if images.is_empty() {
                                state_ref.runtime.run_turn(&text, prompter)
                            } else {
                                // Multimodal turn: images first, then text.
                                let mut blocks: Vec<runtime::ContentBlock> = images
                                    .iter()
                                    .map(|img| runtime::ContentBlock::Image {
                                        media_type: img.media_type.clone(),
                                        data: img.data.clone(),
                                    })
                                    .collect();
                                if !text.is_empty() {
                                    blocks.push(runtime::ContentBlock::Text { text: text.clone() });
                                }
                                state_ref.runtime.run_turn_with_content(blocks, prompter)
                            }
                        }),
                    );

                    match panic_result {
                        Ok(Ok(summary)) => {
                            let reply = extract_text(&summary);
                            eprintln!("[worker] run_turn OK, reply_len={}, tokens={}/{}",
                                reply.len(), summary.usage.input_tokens, summary.usage.output_tokens);
                            // Best-effort token log append for stats panel.
                            // Count tool_result blocks marked `is_error` —
                            // quality signal feeding into the quality panel.
                            let tool_error_count: u32 = summary
                                .tool_results
                                .iter()
                                .flat_map(|m| m.blocks.iter())
                                .filter(|b| {
                                    matches!(
                                        b,
                                        runtime::ContentBlock::ToolResult {
                                            is_error: true,
                                            ..
                                        }
                                    )
                                })
                                .count()
                                .try_into()
                                .unwrap_or(u32::MAX);
                            let iterations: u32 =
                                summary.iterations.try_into().unwrap_or(u32::MAX);
                            let _ = token_stats::record_turn_full(
                                &cfg.model,
                                u64::from(summary.usage.input_tokens),
                                u64::from(summary.usage.output_tokens),
                                Some(iterations),
                                Some(tool_error_count),
                            );
                            // Emit context-health event so the UI badge can
                            // surface "context rot" risk before the model
                            // starts degrading (typically begins ~20-40% fill).
                            let post_turn_fill = {
                                let input_t = u64::from(summary.usage.input_tokens);
                                let window = u64::from(
                                    context_window::context_window_tokens(&cfg.model),
                                );
                                let ratio = context_window::fill_ratio(input_t, &cfg.model);
                                let _ = app_handle.emit(
                                    "context-health",
                                    serde_json::json!({
                                        "input_tokens": input_t,
                                        "window": window,
                                        "fill_ratio": ratio,
                                        "model": cfg.model,
                                    }),
                                );
                                ratio
                            };

                            // Auto-compact: when the user has opted in and
                            // the context is above the configured threshold,
                            // run a compaction pass in the background so the
                            // next turn sees a slimmer working set. Failures
                            // are swallowed — compaction is best-effort.
                            if cfg.auto_compact_threshold > 0.0
                                && post_turn_fill >= cfg.auto_compact_threshold
                            {
                                if let Some(state_ref) = desktop_state.as_mut() {
                                    let session = state_ref.runtime.session_mut();
                                    match compaction::compact_session(session, &cfg) {
                                        Ok(Some(report)) => {
                                            eprintln!(
                                                "[worker] auto-compaction dropped {} kept {}",
                                                report.dropped_message_count,
                                                report.kept_message_count
                                            );
                                            let _ = app_handle.emit(
                                                "compaction-done",
                                                serde_json::json!({
                                                    "dropped": report.dropped_message_count,
                                                    "kept": report.kept_message_count,
                                                    "summary_excerpt":
                                                        report.summary.chars().take(160)
                                                            .collect::<String>(),
                                                    "auto": true,
                                                }),
                                            );
                                        }
                                        Ok(None) => {
                                            eprintln!("[worker] auto-compaction skipped (no safe cut)");
                                        }
                                        Err(e) => {
                                            eprintln!("[worker] auto-compaction failed: {e}");
                                        }
                                    }
                                }
                            }
                            hooks::dispatch(
                                &cfg.hooks,
                                &hooks::HookContext::new(
                                    "after_turn",
                                    current_session_id.clone(),
                                )
                                .with_extra(
                                    "input_tokens",
                                    serde_json::json!(u64::from(summary.usage.input_tokens)),
                                )
                                .with_extra(
                                    "output_tokens",
                                    serde_json::json!(u64::from(summary.usage.output_tokens)),
                                )
                                .with_extra(
                                    "reply_excerpt",
                                    serde_json::json!(
                                        reply.chars().take(200).collect::<String>()
                                    ),
                                ),
                            );
                            Ok(TurnResult {
                                text: reply,
                                input_tokens: summary.usage.input_tokens,
                                output_tokens: summary.usage.output_tokens,
                            })
                        }
                        Ok(Err(e)) => {
                            let msg = e.to_string();
                            if msg.contains(api_client::CANCELLED_MARKER) {
                                eprintln!("[worker] turn cancelled by user");
                                Err("__CANCELLED__".to_string())
                            } else if msg.contains("conversation loop exceeded") {
                                // Replace technical message with actionable
                                // guidance. Common cause: model stuck in tool
                                // loop without converging on a final answer.
                                eprintln!("[worker] iteration limit hit: {msg}");
                                Err(format!(
                                    "对话循环超过 {} 轮上限。可能原因：模型陷入工具调用循环没有产出最终答复。\n\
                                     建议：1) 简化任务拆分为更小的子任务；2) 让 CEO 直接回答而不要委派；\n\
                                     3) 通过环境变量 CLAWD_CEO_MAX_ITERATIONS 调高上限。",
                                    ceo_iter_limit_for_display()
                                ))
                            } else {
                                eprintln!("[worker] run_turn error: {msg}");
                                Err(msg)
                            }
                        }
                        Err(panic_payload) => {
                            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic payload".to_string()
                            };
                            eprintln!("[worker] PANIC during run_turn: {panic_msg}");
                            // Runtime state may be poisoned — rebuild so the
                            // next turn starts fresh. User loses session
                            // history but not the app.
                            desktop_state = DesktopState::build(
                                &cfg,
                                cancel_flag.clone(),
                                event_sink::tauri_sink(app_handle.clone()),
                                current_session_id.clone(),
                            )
                            .ok();
                            Err(format!(
                                "Internal panic recovered ({panic_msg}). Session was reset; \
                                 please retry your message."
                            ))
                        }
                    }
                } else {
                    eprintln!("[worker] desktop_state is None");
                    Err("API Key not configured. Please open Settings to add your API key and model.".to_string())
                };
                let _ = responder.send(result);
            }
            WorkerMsg::ClearSession { responder } => {
                // "Clear" semantics under the multi-session model: delete the
                // current session's jsonl file AND start a new one with a
                // fresh id. This keeps other sessions untouched.
                let old_path = state::session_jsonl_path(&current_session_id);
                if old_path.exists() {
                    if let Err(e) = std::fs::remove_file(&old_path) {
                        eprintln!("[worker] failed to remove session file: {e}");
                    }
                }
                // Also drop the pinned-decisions file for this session so
                // anchors don't outlive the conversation that created them.
                if let Err(e) = anchors::clear(&current_session_id) {
                    eprintln!("[worker] failed to clear anchors: {e}");
                }
                let new_id = state::new_session_id();
                let _ = state::set_current_session_id(&new_id);
                current_session_id = new_id;

                let result = DesktopState::build(
                    &cfg,
                    cancel_flag.clone(),
                    event_sink::tauri_sink(app_handle.clone()),
                    current_session_id.clone(),
                )
                .map(|s| {
                    desktop_state = Some(s);
                })
                .map_err(|e| e.to_string());
                let _ = app_handle.emit("session-changed", &current_session_id);
                let _ = responder.send(result);
            }
            WorkerMsg::SwitchSession { new_id, responder } => {
                eprintln!("[worker] SwitchSession to {new_id}");
                let _ = state::set_current_session_id(&new_id);
                current_session_id = new_id;

                let result = DesktopState::build(
                    &cfg,
                    cancel_flag.clone(),
                    event_sink::tauri_sink(app_handle.clone()),
                    current_session_id.clone(),
                )
                .map(|s| {
                    desktop_state = Some(s);
                })
                .map_err(|e| e.to_string());
                let _ = app_handle.emit("session-changed", &current_session_id);
                let _ = responder.send(result);
            }
            WorkerMsg::CompactSession { responder } => {
                let result = if let Some(state_ref) = desktop_state.as_mut() {
                    let session = state_ref.runtime.session_mut();
                    compaction::compact_session(session, &cfg)
                } else {
                    Err("worker has no active session to compact".to_string())
                };
                if let Ok(Some(ref report)) = result {
                    eprintln!(
                        "[worker] compaction done: dropped {} kept {}",
                        report.dropped_message_count, report.kept_message_count
                    );
                    let _ = app_handle.emit(
                        "compaction-done",
                        serde_json::json!({
                            "dropped": report.dropped_message_count,
                            "kept": report.kept_message_count,
                            "summary_excerpt": report.summary.chars().take(160).collect::<String>(),
                            "auto": false,
                        }),
                    );
                }
                let _ = responder.send(result);
            }
            WorkerMsg::Reinitialize { config, responder } => {
                eprintln!("[worker] Reinitialize: model={:?} api_key_set={} base_url={:?}",
                    config.model, !config.api_key.is_empty(), config.base_url);
                apply_config_to_env(&config);
                cfg = config;
                // A previous turn may have been cancelled (flag=true). Reset
                // it now so the next turn after settings-save isn't immediately
                // short-circuited by a stale cancellation signal.
                cancel_flag.store(false, std::sync::atomic::Ordering::SeqCst);
                let result = DesktopState::build(
                    &cfg,
                    cancel_flag.clone(),
                    event_sink::tauri_sink(app_handle.clone()),
                    current_session_id.clone(),
                )
                .map(|s| {
                    eprintln!("[worker] Reinitialize: DesktopState built OK");
                    desktop_state = Some(s);
                })
                .map_err(|e| {
                    eprintln!("[worker] Reinitialize error: {e}");
                    e.to_string()
                });
                let _ = responder.send(result);
            }
        }
    }
}

// ── App entry point ───────────────────────────────────────────────────────────

pub fn run() {
    // Detect long tasks left in "Running" state from a previous session
    // whose worker crashed / was force-quit. Mark them Interrupted so the
    // UI surfaces them and the user can decide what to do.
    match long_task::reap_interrupted() {
        Ok(0) => {}
        Ok(n) => eprintln!("[startup] marked {n} interrupted long task(s)"),
        Err(e) => eprintln!("[startup] reap_interrupted failed: {e}"),
    }

    let (tx, rx) = std::sync::mpsc::sync_channel::<WorkerMsg>(32);
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_for_worker = cancel_flag.clone();
    let rx_holder = std::sync::Mutex::new(Some(rx));

    let long_task_cancels = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::HashMap::new(),
    ));
    let app_state = AppState {
        tx,
        cancel_flag,
        long_task_cancels,
    };

    tauri::Builder::default()
        .setup(move |app| {
            // Spawn the worker once we have an AppHandle. The worker emits
            // streaming events (`turn-stream-*`) directly via this handle so
            // the UI sees text deltas and tool calls in real time.
            let handle = app.handle().clone();
            let cancel = cancel_for_worker.clone();
            if let Some(rx) = rx_holder.lock().ok().and_then(|mut g| g.take()) {
                std::thread::spawn(move || worker_loop(rx, cancel, handle));
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            send_message,
            list_opc_agents,
            clear_session,
            get_settings,
            save_settings,
            read_agent_detail,
            dismiss_agent,
            clear_agents,
            cancel_turn,
            restore_session,
            export_session,
            write_export,
            list_sessions,
            current_session_id,
            switch_session,
            new_session,
            delete_session,
            get_token_stats,
            start_long_task,
            resume_long_task,
            list_long_tasks,
            get_long_task,
            read_long_task_output,
            cancel_long_task,
            delete_long_task,
            run_dream,
            apply_dream,
            list_memory_files,
            run_agent_profile_dream,
            apply_agent_profile,
            read_attachment,
            list_slash_commands,
            list_skills,
            create_skill,
            delete_skill,
            toggle_skill,
            read_skill,
            list_remote_skills,
            import_remote_skill,
            import_local_skill,
            get_mcp_status,
            get_cost_breakdown,
            get_quality_breakdown,
            compact_session_now,
            list_anchors,
            remove_anchor,
            get_company_context,
            save_company_context,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
