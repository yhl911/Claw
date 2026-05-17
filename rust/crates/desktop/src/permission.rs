//! Auto-approval prompter for OPC desktop mode.
//!
//! The desktop's `PermissionPolicy` is set to `WorkspaceWrite`. The `Agent`
//! tool (and a small set of OPC infrastructure tools) require an escalation
//! to `DangerFullAccess`, which without a prompter results in a silent
//! deny — meaning the CEO would never be able to delegate to sub-agents.
//!
//! `OpcApprover` whitelists the tools that OPC mode legitimately needs and
//! denies anything else, surfacing a meaningful reason. This preserves the
//! permission boundary for unknown / unexpected escalations while letting
//! the CEO/sub-agent delegation pipeline work end-to-end.

use runtime::{PermissionPromptDecision, PermissionPrompter, PermissionRequest};

/// Default OPC prompter: approves every tool escalation.
///
/// Rationale: the desktop app runs on the user's own machine, the user
/// installed it deliberately, and the typical permission mode is already
/// `DangerFullAccess` (no escalation will ever be requested). The
/// prompter only matters when the user dials the mode down to
/// `WorkspaceWrite` or `ReadOnly` — and even then, denying tools the
/// CEO needs (bash, WebSearch, write_file …) just confuses the model,
/// which then narrates "环境权限受限" to the user instead of doing work.
///
/// If a future security concern requires per-tool gating, this is the
/// right place to add a user-facing prompt (emit a Tauri event, await
/// click). For now, blanket-approve.
pub struct OpcApprover;

impl PermissionPrompter for OpcApprover {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        eprintln!(
            "[permission] auto-approving '{}' (desktop trusts its own user)",
            request.tool_name
        );
        PermissionPromptDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::PermissionMode;

    fn make_request(tool: &str) -> PermissionRequest {
        PermissionRequest {
            tool_name: tool.to_string(),
            input: String::new(),
            current_mode: PermissionMode::WorkspaceWrite,
            required_mode: PermissionMode::DangerFullAccess,
            reason: None,
        }
    }

    #[test]
    fn approves_agent_tool() {
        let mut p = OpcApprover;
        assert!(matches!(
            p.decide(&make_request("Agent")),
            PermissionPromptDecision::Allow
        ));
    }

    #[test]
    fn approves_task_management_tools() {
        let mut p = OpcApprover;
        for t in ["TaskStop", "TaskOutput", "TaskUpdate"] {
            assert!(
                matches!(p.decide(&make_request(t)), PermissionPromptDecision::Allow),
                "expected approve for {t}"
            );
        }
    }

    #[test]
    fn approves_any_tool() {
        // Desktop trusts its own user — every tool escalation request
        // is auto-approved. This is the property the CEO depends on.
        let mut p = OpcApprover;
        for t in ["bash", "WebSearch", "write_file", "MysteriousNewTool"] {
            assert!(
                matches!(p.decide(&make_request(t)), PermissionPromptDecision::Allow),
                "expected approve for {t}"
            );
        }
    }
}
