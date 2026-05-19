//! Runner that executes a `LongTask` on a dedicated worker thread.
//!
//! Differences vs. the conversational `worker_loop`:
//! - **No iteration cap from runtime defaults** — uses task spec's
//!   `max_total_iterations` (default 2000) instead of the CEO/sub-agent
//!   200/80 caps that exist to detect runaway conversations.
//! - **Heartbeat thread** writes `state.json` every 30s so the recovery
//!   logic can tell whether the worker is alive.
//! - **Per-task session jsonl** under `long_tasks/<id>/messages.jsonl` so
//!   the conversation history is isolated and survives restarts.
//! - **Persistent status** via `TaskStatus` (Pending → Running → Done /
//!   Failed / Cancelled / Interrupted).
//!
//! Limitations of this initial cut (will be addressed in Phase 2):
//! - No automatic retry on transient errors. A network blip mid-run
//!   currently fails the task; the user can manually start a new task.
//! - No automatic resume after Interrupted. The user sees the
//!   interrupted task and decides what to do.
//! - Single-task at a time per session (worker is single-threaded).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use runtime::{ContentBlock, PermissionPolicy, Session};

use crate::api_client::{DesktopApiClient, CANCELLED_MARKER};
use crate::config::{normalize_model, DesktopConfig};
use crate::event_sink::Sink;
use crate::long_task::{
    self, messages_path, output_path, save_state, HEARTBEAT_INTERVAL_SECS, TaskState, TaskStatus,
};
use crate::permission::OpcApprover;
use crate::tool_executor::DesktopToolExecutor;

/// Run a long task end to end. Blocks the calling thread for the entire
/// duration of the task — meant to be invoked from a dedicated worker
/// thread (the regular conversational worker, or its own).
///
/// `cancel_flag` is shared with the Tauri layer so the user can request
/// cancellation. The runner re-uses the same cancellation propagation as
/// the conversational path: `DesktopApiClient` checks the flag between
/// SSE events and bails with `CANCELLED_MARKER`.
///
/// `resume` distinguishes "fresh start" (push the original goal as the
/// first user message) from "continuation" (load existing session jsonl
/// and nudge the model to continue from where it stopped).
pub fn run(
    task_id: &str,
    cfg: &DesktopConfig,
    cancel_flag: Arc<AtomicBool>,
    sink: Sink,
    resume: bool,
) -> Result<(), String> {
    let spec = long_task::load_spec(task_id).map_err(|e| format!("load spec: {e}"))?;

    // Budget gate. A long task can run for hours and cost real money;
    // refuse to start (or resume) if the user's daily/monthly cap is
    // already at zero. We rely on the same trailing-window cost
    // computation as the worker so behavior is consistent.
    if let Err(reason) = crate::token_stats::check_budget(
        cfg.budget_daily_usd,
        cfg.budget_monthly_usd,
    ) {
        mark_failed(task_id, &format!("budget: {reason}"))?;
        return Err(reason);
    }

    // Mark Running and emit so the UI re-renders the row.
    {
        let mut s = long_task::load_state(task_id).map_err(|e| format!("load state: {e}"))?;
        s.status = TaskStatus::Running;
        s.started_at = Some(long_task::now_secs());
        s.last_heartbeat = long_task::now_secs();
        save_state(&s).map_err(|e| format!("save running state: {e}"))?;
    }
    sink.emit("long-task-changed", serde_json::Value::String(task_id.to_string()));

    // Heartbeat thread keeps `state.last_heartbeat` fresh while the
    // (potentially very long) run_turn call is in flight.
    let hb_alive = Arc::new(AtomicBool::new(true));
    let hb_alive_inner = hb_alive.clone();
    let hb_task_id = task_id.to_string();
    let hb_handle = std::thread::Builder::new()
        .name(format!("long-task-hb-{task_id}"))
        .spawn(move || {
            while hb_alive_inner.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
                if !hb_alive_inner.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(mut s) = long_task::load_state(&hb_task_id) {
                    s.last_heartbeat = long_task::now_secs();
                    let _ = save_state(&s);
                }
            }
        })
        .map_err(|e| format!("spawn heartbeat: {e}"))?;

    // Build per-task runtime. Note: this runtime is independent of the
    // conversational one — its session lives in long_tasks/<id>/messages.jsonl
    // and its iteration cap comes from the spec, not from defaults.
    let runtime_result = build_task_runtime(&spec.model, cfg, cancel_flag.clone(), sink.clone(), task_id);
    let mut runtime = match runtime_result {
        Ok(r) => r,
        Err(e) => {
            hb_alive.store(false, Ordering::SeqCst);
            let _ = hb_handle.join();
            mark_failed(task_id, &format!("build runtime: {e}"))?;
            sink.emit("long-task-changed", serde_json::Value::String(task_id.to_string()));
            return Err(e);
        }
    };

    eprintln!(
        "[long-runner] starting task '{}' (max_iter={:?}, goal_chars={})",
        task_id,
        spec.max_total_iterations,
        spec.goal.chars().count()
    );
    let started = Instant::now();

    // Wrapped in catch_unwind so any panic in runtime / api / tools doesn't
    // poison the worker thread that drives long tasks.
    let mut approver = OpcApprover;
    let prompter: Option<&mut dyn runtime::PermissionPrompter> = Some(&mut approver);
    // Fresh start vs resume:
    // - Fresh: push the task's original goal as the first user message
    //   (run_turn always appends user_input to session.messages).
    // - Resume: session jsonl is already loaded with prior context; we
    //   nudge the model with a short continuation prompt so it picks up
    //   where it stopped. Otherwise (empty/duplicate prompt) the runtime
    //   would either complain or repeat the goal.
    let turn_input = if resume {
        eprintln!(
            "[long-runner] task '{task_id}' is RESUMING — sending continuation prompt"
        );
        "继续之前的任务，基于已有上下文从中断处接着工作直到完成。\
         不需要重新分析问题，直接基于已经做过的内容继续。"
            .to_string()
    } else {
        spec.goal.clone()
    };
    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.run_turn(turn_input, prompter)
    }));

    // Always stop heartbeat before writing terminal state, so we don't race.
    hb_alive.store(false, Ordering::SeqCst);
    let _ = hb_handle.join();

    let elapsed = started.elapsed();
    let outcome: Result<(), String> = match panic_result {
        Ok(Ok(summary)) => {
            let final_text = extract_final_text(&summary);
            let total_input = u64::from(summary.usage.input_tokens);
            let total_output = u64::from(summary.usage.output_tokens);
            // Persist output for easy inspection / export.
            let _ = std::fs::write(output_path(task_id), &final_text);
            mark_done(task_id, total_input, total_output, summary.iterations)?;
            eprintln!(
                "[long-runner] task '{task_id}' DONE in {:.1}s, {} iter, {}/{} tokens, {} chars output",
                elapsed.as_secs_f64(),
                summary.iterations,
                total_input,
                total_output,
                final_text.len()
            );
            crate::hooks::dispatch(
                &cfg.hooks,
                &crate::hooks::HookContext::new("after_long_task", String::new())
                    .with_task(task_id.to_string())
                    .with_extra("status", serde_json::json!("done"))
                    .with_extra("iterations", serde_json::json!(summary.iterations))
                    .with_extra("input_tokens", serde_json::json!(total_input))
                    .with_extra("output_tokens", serde_json::json!(total_output))
                    .with_extra(
                        "elapsed_secs",
                        serde_json::json!(elapsed.as_secs_f64()),
                    ),
            );
            sink.emit(
                "long-task-done",
                serde_json::json!({
                    "task_id": task_id,
                    "goal": spec.goal,
                    "elapsed_secs": elapsed.as_secs_f64(),
                    "iterations": summary.iterations,
                }),
            );
            Ok(())
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains(CANCELLED_MARKER) {
                mark_cancelled(task_id)?;
                crate::hooks::dispatch(
                    &cfg.hooks,
                    &crate::hooks::HookContext::new("after_long_task", String::new())
                        .with_task(task_id.to_string())
                        .with_extra("status", serde_json::json!("cancelled")),
                );
                Err("cancelled by user".to_string())
            } else {
                eprintln!("[long-runner] task '{task_id}' FAILED: {msg}");
                mark_failed(task_id, &msg)?;
                crate::hooks::dispatch(
                    &cfg.hooks,
                    &crate::hooks::HookContext::new("after_long_task", String::new())
                        .with_task(task_id.to_string())
                        .with_extra("status", serde_json::json!("failed"))
                        .with_extra("error", serde_json::json!(msg.clone())),
                );
                sink.emit(
                    "long-task-failed",
                    serde_json::json!({
                        "task_id": task_id,
                        "goal": spec.goal,
                        "error": msg,
                    }),
                );
                Err(msg)
            }
        }
        Err(panic) => {
            let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("[long-runner] task '{task_id}' PANICKED: {msg}");
            mark_failed(task_id, &format!("panic: {msg}"))?;
            Err(msg)
        }
    };

    sink.emit("long-task-changed", serde_json::Value::String(task_id.to_string()));
    outcome
}

/// Build a runtime that is wholly independent of the conversational
/// session — its messages persist to the task's own jsonl file.
fn build_task_runtime(
    model: &str,
    cfg: &DesktopConfig,
    cancel_flag: Arc<AtomicBool>,
    sink: Sink,
    task_id: &str,
) -> Result<runtime::ConversationRuntime<DesktopApiClient, DesktopToolExecutor>, String> {
    let model = normalize_model(model);
    let session_path = messages_path(task_id);
    let session = if let Ok(loaded) = Session::load_from_path(&session_path) {
        eprintln!(
            "[long-runner] resuming task '{task_id}' with {} prior message(s)",
            loaded.messages.len()
        );
        loaded
    } else {
        if let Some(parent) = session_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Session::new().with_persistence_path(session_path)
    };

    let provider = api::ProviderClient::from_model(&model).map_err(|e| e.to_string())?;
    let mut tool_specs: Vec<api::ToolDefinition> = tools::mvp_tool_specs()
        .into_iter()
        .map(|spec| api::ToolDefinition {
            name: spec.name.to_string(),
            description: Some(spec.description.to_string()),
            input_schema: spec.input_schema.clone(),
        })
        .collect();

    // Initialize the user-configured MCP servers for this long task too.
    // Without this, a long-running task would silently miss the tools the
    // user expects (e.g. Linear/Slack). Best-effort: log + continue.
    let desktop_mcp = match crate::mcp::init(&cfg.mcp_servers) {
        Ok(opt) => {
            if let Some(ref m) = opt {
                eprintln!("[long-runner] {}", m.status);
                tool_specs.extend(m.tool_specs.clone());
            }
            opt
        }
        Err(e) => {
            eprintln!("[long-runner] MCP init failed: {e}");
            None
        }
    };

    let api_client = DesktopApiClient::new(
        provider,
        model.clone(),
        true,
        tool_specs,
        cfg.thinking_mode,
        cancel_flag,
        sink,
    )
    .map_err(|e| e.to_string())?;

    // Long-running tasks reuse the task_id as their "session id" for
    // anchor scoping — anchors pinned during the task stay with the task.
    let tool_executor =
        DesktopToolExecutor::new(desktop_mcp, task_id.to_string(), cfg.brave_api_key.clone());
    let policy_mode = crate::config::parse_permission_mode(&cfg.permission_mode);
    let policy = PermissionPolicy::new(policy_mode);

    let cwd = std::env::current_dir().unwrap_or_default();
    let date = simple_date();
    let mut system_prompt =
        runtime::load_system_prompt(cwd.clone(), date, std::env::consts::OS, "unknown")
            .unwrap_or_default();
    system_prompt.push(LONG_TASK_PREAMBLE.to_string());

    let skills_section = crate::skills::enabled_skills_prompt_section();
    if !skills_section.is_empty() {
        system_prompt.push(skills_section);
    }

    let max_iter = long_task::load_spec(task_id)
        .ok()
        .and_then(|s| s.max_total_iterations)
        .unwrap_or(long_task::DEFAULT_MAX_ITERATIONS);

    Ok(runtime::ConversationRuntime::new(
        session,
        api_client,
        tool_executor,
        policy,
        system_prompt,
    )
    .with_max_iterations(max_iter))
}

/// System-prompt nudge specific to long-running tasks. Reinforces "this
/// is a long autonomous run, you have headroom but converge eventually".
const LONG_TASK_PREAMBLE: &str = "\
## 长跑任务模式\n\n\
你正在一个长跑任务模式下工作。你有充足的预算和迭代次数（默认 2000 轮），\
但**目标是完成任务并产出最终交付物**，不是无限制探索。\n\n\
原则：\n\
- 任务完成时，写一段总结性的最终回答（这是任务输出）\n\
- 遇到不确定时，做合理假设并继续，不要卡住\n\
- 复杂任务可以多次工具调用、跨多轮迭代，没有强约束，但要有进展\n\
- 最终回答要包含：完成了什么 / 关键决策 / 已知限制\n";

fn simple_date() -> String {
    let secs = long_task::now_secs();
    format!("{}-01-01", 1970 + secs / 86400 / 365)
}

fn extract_final_text(summary: &runtime::TurnSummary) -> String {
    summary
        .assistant_messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn mark_done(
    task_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    iterations: usize,
) -> Result<(), String> {
    let mut s = long_task::load_state(task_id).map_err(|e| e.to_string())?;
    s.status = TaskStatus::Done;
    s.completed_at = Some(long_task::now_secs());
    s.last_heartbeat = long_task::now_secs();
    s.input_tokens += input_tokens;
    s.output_tokens += output_tokens;
    s.current_iteration = iterations;
    save_state(&s).map_err(|e| e.to_string())
}

fn mark_failed(task_id: &str, error: &str) -> Result<(), String> {
    let mut s = match long_task::load_state(task_id) {
        Ok(s) => s,
        Err(_) => {
            // No state file yet — make a minimal failed record.
            TaskState {
                task_id: task_id.to_string(),
                status: TaskStatus::Failed,
                current_iteration: 0,
                input_tokens: 0,
                output_tokens: 0,
                started_at: None,
                last_heartbeat: long_task::now_secs(),
                completed_at: Some(long_task::now_secs()),
                last_error: Some(error.to_string()),
                retry_count: 0,
            }
        }
    };
    s.status = TaskStatus::Failed;
    s.completed_at = Some(long_task::now_secs());
    s.last_heartbeat = long_task::now_secs();
    s.last_error = Some(error.to_string());
    save_state(&s).map_err(|e| e.to_string())
}

fn mark_cancelled(task_id: &str) -> Result<(), String> {
    let mut s = long_task::load_state(task_id).map_err(|e| e.to_string())?;
    s.status = TaskStatus::Cancelled;
    s.completed_at = Some(long_task::now_secs());
    s.last_heartbeat = long_task::now_secs();
    s.last_error = Some("Cancelled by user".to_string());
    save_state(&s).map_err(|e| e.to_string())
}
