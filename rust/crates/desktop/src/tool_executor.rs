use runtime::{ToolError, ToolExecutor};

use crate::anchors::{self, AnchorEntry};
use crate::loop_detector::LoopDetector;
use crate::mcp::DesktopMcp;

pub struct DesktopToolExecutor {
    /// Optional MCP integration. When present, tool names starting with
    /// `mcp__` are routed to the MCP manager instead of the built-in
    /// global tool registry.
    mcp: Option<DesktopMcp>,
    /// Detects repeated identical tool calls within the same conversation
    /// turn and short-circuits with a tool error to break the loop.
    loop_detector: LoopDetector,
    /// Active session id — used by desktop-injected tools (notably
    /// `pin_decision`) to scope their side effects to the right session.
    session_id: String,
}

impl DesktopToolExecutor {
    pub fn new(mcp: Option<DesktopMcp>, session_id: String) -> Self {
        Self {
            mcp,
            loop_detector: LoopDetector::new(),
            session_id,
        }
    }

    /// Reset per-turn state. Must be called before each `run_turn` so the
    /// loop detector's ring buffer doesn't accumulate calls across different
    /// user turns (which would cause false-positive loop detection).
    pub fn reset_for_new_turn(&mut self) {
        self.loop_detector.reset();
    }

    /// Handle the `pin_decision` desktop tool. Returns `None` if the tool
    /// name doesn't match, so the caller can fall through to other paths.
    fn try_pin_decision(&self, tool_name: &str, input: &str) -> Option<Result<String, ToolError>> {
        if tool_name != "pin_decision" {
            return None;
        }
        let parsed: serde_json::Value = match serde_json::from_str(input) {
            Ok(v) => v,
            Err(e) => {
                return Some(Err(ToolError::new(format!(
                    "pin_decision: invalid input JSON: {e}"
                ))));
            }
        };
        let title = parsed
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let rationale = parsed
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() || rationale.is_empty() {
            return Some(Err(ToolError::new(
                "pin_decision: both `title` and `rationale` are required".to_string(),
            )));
        }
        let entry = AnchorEntry {
            title: title.clone(),
            rationale: rationale.clone(),
            pinned_at_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        if let Err(e) = anchors::append(&self.session_id, entry) {
            return Some(Err(ToolError::new(format!(
                "pin_decision: failed to persist anchor: {e}"
            ))));
        }
        Some(Ok(format!(
            "📌 Pinned decision: \"{title}\". It will be reinjected into the system \
             prompt for every subsequent turn this session. Rationale: {rationale}"
        )))
    }

    /// Try MCP first if the tool name is in our registered map. Returns
    /// `None` to signal "fall through to built-in dispatch".
    fn try_mcp(&self, tool_name: &str, input: &str) -> Option<Result<String, ToolError>> {
        let mcp = self.mcp.as_ref()?;
        if !mcp.name_map.contains_key(tool_name) {
            return None;
        }
        let value: serde_json::Value = match serde_json::from_str(input) {
            Ok(v) => v,
            Err(e) => {
                return Some(Err(ToolError::new(format!(
                    "invalid MCP tool input JSON: {e}"
                ))));
            }
        };
        Some(mcp.call(tool_name, &value).map_err(ToolError::new))
    }
}

impl ToolExecutor for DesktopToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if let Some(loop_msg) = self.loop_detector.record(tool_name, input) {
            eprintln!("[tool_executor] loop detector tripped for {tool_name}");
            return Err(ToolError::new(loop_msg));
        }
        if let Some(result) = self.try_pin_decision(tool_name, input) {
            return result;
        }
        if let Some(result) = self.try_mcp(tool_name, input) {
            return result;
        }
        let input_value: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| ToolError::new(format!("invalid tool input JSON: {e}")))?;
        tools::execute_tool(tool_name, &input_value).map_err(ToolError::new)
    }

    /// Override the default sequential implementation: dispatch each call on
    /// its own scoped thread so that long-running tools (notably `Agent` —
    /// which blocks for 30s~3min per sub-agent under sync semantics) can run
    /// in parallel when the model emits multiple tool_use blocks in one
    /// assistant message.
    ///
    /// MCP-routed calls are intentionally serialized (one mutex on the
    /// manager); only built-in tools fan out. Mixing is fine — built-in
    /// runs in scoped thread, MCP runs on the calling thread.
    fn execute_batch(
        &mut self,
        calls: &[(String, String)],
    ) -> Vec<Result<String, ToolError>> {
        // Fast path: zero or one calls — skip thread setup.
        if calls.len() <= 1 {
            return calls
                .iter()
                .map(|(name, input)| self.execute(name, input))
                .collect();
        }

        // Loop detector pass: feed each call through the detector first.
        // If any trips, return loop-error for that slot; remaining calls
        // still execute (model may have mixed a useful call into the
        // batch). Important: this must happen sequentially so the
        // detector's ring buffer sees calls in order.
        let mut loop_block: Vec<Option<String>> = Vec::with_capacity(calls.len());
        for (name, input) in calls {
            loop_block.push(self.loop_detector.record(name, input));
        }

        eprintln!(
            "[tool_executor] running {} tool calls in parallel",
            calls.len()
        );

        // Pre-compute which calls are MCP so we can run them serially (the
        // manager has a single mutex) and only thread-fan-out the rest.
        let is_mcp: Vec<bool> = calls
            .iter()
            .map(|(name, _)| {
                self.mcp
                    .as_ref()
                    .is_some_and(|m| m.name_map.contains_key(name))
            })
            .collect();

        // Run all MCP calls serially (cheap: usually 0 or 1 of them) and
        // store results indexed by position. Loop-tripped slots are
        // filled with a ToolError now and never dispatched.
        let mut results: Vec<Option<Result<String, ToolError>>> =
            (0..calls.len()).map(|_| None).collect();
        for (i, (name, input)) in calls.iter().enumerate() {
            if let Some(msg) = loop_block[i].take() {
                eprintln!("[tool_executor] loop detector tripped for {name} (batch slot {i})");
                results[i] = Some(Err(ToolError::new(msg)));
                continue;
            }
            if let Some(r) = self.try_pin_decision(name, input) {
                results[i] = Some(r);
                continue;
            }
            if is_mcp[i] {
                results[i] = self.try_mcp(name, input);
            }
        }

        // Run the rest in parallel scoped threads. Skip slots already
        // resolved by the loop detector.
        let pending: Vec<(usize, &(String, String))> = calls
            .iter()
            .enumerate()
            .filter(|(i, _)| !is_mcp[*i] && results[*i].is_none())
            .collect();

        let parallel_results: Vec<(usize, Result<String, ToolError>)> = std::thread::scope(|s| {
            let handles: Vec<_> = pending
                .iter()
                .map(|(idx, (name, input))| {
                    let idx = *idx;
                    let name = name.clone();
                    let input = input.clone();
                    (
                        idx,
                        s.spawn(move || -> Result<String, ToolError> {
                            let input_value: serde_json::Value = serde_json::from_str(&input)
                                .map_err(|e| {
                                    ToolError::new(format!("invalid tool input JSON: {e}"))
                                })?;
                            tools::execute_tool(&name, &input_value).map_err(ToolError::new)
                        }),
                    )
                })
                .collect();

            handles
                .into_iter()
                .map(|(idx, h)| {
                    let r = match h.join() {
                        Ok(result) => result,
                        Err(_) => Err(ToolError::new(
                            "tool worker thread panicked".to_string(),
                        )),
                    };
                    (idx, r)
                })
                .collect()
        });

        for (idx, r) in parallel_results {
            results[idx] = Some(r);
        }

        results
            .into_iter()
            .map(|opt| {
                opt.unwrap_or_else(|| {
                    Err(ToolError::new(
                        "internal: result slot not filled".to_string(),
                    ))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Verify that `execute_batch` actually runs calls concurrently. We pick
    /// `bash` with `sleep 1` × 3 — sequential would take ~3s, parallel ~1s.
    /// This is a real wall-clock test so we accept anything under 2s as
    /// "parallel" and anything over 2.5s as "sequential" (with margin).
    #[test]
    fn execute_batch_runs_in_parallel() {
        let mut exec = DesktopToolExecutor::new(None, "test-session".to_string());
        // Use distinct command suffixes so the LoopDetector (3 identical
        // calls in a row trips an error) doesn't fire on this batch.
        let calls = vec![
            (
                "bash".to_string(),
                serde_json::json!({"command": "sleep 1 && echo a"}).to_string(),
            ),
            (
                "bash".to_string(),
                serde_json::json!({"command": "sleep 1 && echo b"}).to_string(),
            ),
            (
                "bash".to_string(),
                serde_json::json!({"command": "sleep 1 && echo c"}).to_string(),
            ),
        ];

        let start = Instant::now();
        let results = exec.execute_batch(&calls);
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 3, "must return one result per call");
        for r in &results {
            assert!(r.is_ok(), "bash sleep should succeed: {r:?}");
        }
        assert!(
            elapsed < Duration::from_millis(2_500),
            "execute_batch with 3x `sleep 1` ran in {elapsed:?}, expected < 2.5s if parallel"
        );
    }

    #[test]
    fn execute_batch_fast_path_for_single_call() {
        // Single-call path skips thread::scope but still works.
        let mut exec = DesktopToolExecutor::new(None, "test-session".to_string());
        let calls = vec![(
            "bash".to_string(),
            serde_json::json!({"command": "echo hello"}).to_string(),
        )];
        let results = exec.execute_batch(&calls);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
        assert!(results[0].as_ref().unwrap().contains("hello"));
    }

    #[test]
    fn execute_batch_preserves_order_with_mixed_durations() {
        // Even though calls run concurrently, results must come back in input order.
        let mut exec = DesktopToolExecutor::new(None, "test-session".to_string());
        let calls = vec![
            (
                "bash".to_string(),
                serde_json::json!({"command": "sleep 0.5 && echo first"}).to_string(),
            ),
            (
                "bash".to_string(),
                serde_json::json!({"command": "echo second"}).to_string(),
            ),
        ];
        let results = exec.execute_batch(&calls);
        assert!(results[0].as_ref().unwrap().contains("first"));
        assert!(results[1].as_ref().unwrap().contains("second"));
    }
}
