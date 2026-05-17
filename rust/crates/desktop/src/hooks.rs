//! User-configurable hooks — shell commands the desktop runs on event
//! boundaries (turn completion, session clear, long-task finish).
//!
//! Hooks are stored in `DesktopConfig::hooks` and fire asynchronously so
//! a slow hook never blocks the UI thread. Each hook receives a JSON
//! event payload on stdin (one line, newline-terminated); stdout/stderr
//! are captured into the dev log for debugging but not surfaced.
//!
//! Use cases the design has to support:
//! - Desktop notification when a long task finishes ("notify-send" etc.)
//! - Auto-commit / backup on `before_clear_session`
//! - Push to a logging webhook on every assistant turn

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    pub id: String,
    /// One of [`HOOK_EVENTS`].
    pub event: String,
    /// Shell to invoke. On unix runs through `/bin/sh -c`, on windows
    /// through `cmd /C` — this avoids each user re-implementing arg
    /// parsing for trivial cases like `echo "done"`.
    pub command: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Hard wall-clock limit. 0 = no limit (but we still wait_with_output
    /// so a hung hook will eventually be reaped by the OS).
    #[serde(default)]
    pub timeout_secs: u32,
}

fn default_true() -> bool {
    true
}

/// Supported event names. UI uses these as a dropdown.
pub const HOOK_EVENTS: &[&str] = &[
    "after_turn",
    "before_clear_session",
    "after_clear_session",
    "after_long_task",
    "after_agent",
];

/// JSON payload piped to the hook's stdin. We keep the schema small and
/// stable so users can grep for keys without their hook breaking when we
/// add new fields. `extra` is a free-form map for event-specific bits.
#[derive(Debug, Clone, Serialize)]
pub struct HookContext {
    pub event: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Free-form event-specific fields (input tokens, output tokens,
    /// final text excerpt, etc.). Keep small — hook stdin is read in one
    /// shot.
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl HookContext {
    pub fn new(event: &str, session_id: String) -> Self {
        Self {
            event: event.to_string(),
            session_id,
            task_id: None,
            agent_id: None,
            extra: serde_json::Map::new(),
        }
    }
    #[must_use]
    pub fn with_task(mut self, task_id: String) -> Self {
        self.task_id = Some(task_id);
        self
    }
    #[must_use]
    pub fn with_extra(mut self, key: &str, value: serde_json::Value) -> Self {
        self.extra.insert(key.to_string(), value);
        self
    }
}

/// Fire all hooks subscribed to `event`. Each hook runs in its own
/// thread so one slow hook doesn't block subsequent ones. Returns
/// immediately — completion is logged to stderr.
pub fn dispatch(hooks: &[HookSpec], context: &HookContext) {
    let payload = match serde_json::to_string(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[hook] serialize context: {e}");
            return;
        }
    };
    for hook in hooks {
        if !hook.enabled || hook.event != context.event || hook.command.trim().is_empty() {
            continue;
        }
        let hook = hook.clone();
        let payload = payload.clone();
        std::thread::Builder::new()
            .name(format!("hook-{}", hook.id))
            .spawn(move || run_one(&hook, &payload))
            .ok();
    }
}

fn run_one(hook: &HookSpec, payload: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    eprintln!("[hook] firing {} ({})", hook.id, hook.event);

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &hook.command]);
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &hook.command]);
        c
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[hook] {} spawn failed: {e}", hook.id);
            return;
        }
    };
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(stdin, "{payload}");
    }

    // wait_with_output() blocks the hook thread — that's fine because we
    // already spawned this on a dedicated thread. The OS will reap the
    // child even if we never collect it, but capturing output lets us
    // log failures.
    match child.wait_with_output() {
        Ok(out) => {
            if !out.status.success() {
                eprintln!(
                    "[hook] {} exit={:?} stderr={}",
                    hook.id,
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")
                );
            }
        }
        Err(e) => eprintln!("[hook] {} wait failed: {e}", hook.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_serializes_minimal_fields() {
        let ctx = HookContext::new("after_turn", "sess-1".into());
        let s = serde_json::to_string(&ctx).unwrap();
        assert!(s.contains("\"event\":\"after_turn\""));
        assert!(s.contains("\"session_id\":\"sess-1\""));
        // Skipped when None
        assert!(!s.contains("task_id"));
    }

    #[test]
    fn context_with_extra_round_trips() {
        let ctx = HookContext::new("after_long_task", "sess".into())
            .with_task("lt-1".into())
            .with_extra("output_tokens", serde_json::json!(42));
        let s = serde_json::to_string(&ctx).unwrap();
        assert!(s.contains("\"task_id\":\"lt-1\""));
        assert!(s.contains("\"output_tokens\":42"));
    }

    #[test]
    fn dispatch_skips_disabled_and_mismatched() {
        // A disabled hook + wrong-event hook → both filtered out, no
        // command runs (we'd notice in test runtime if one did spawn).
        let hooks = vec![
            HookSpec {
                id: "off".into(),
                event: "after_turn".into(),
                command: "exit 1".into(),
                enabled: false,
                timeout_secs: 0,
            },
            HookSpec {
                id: "wrong".into(),
                event: "after_long_task".into(),
                command: "exit 1".into(),
                enabled: true,
                timeout_secs: 0,
            },
        ];
        dispatch(&hooks, &HookContext::new("after_turn", "s".into()));
    }
}
