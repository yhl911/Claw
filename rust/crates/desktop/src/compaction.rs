//! Session compaction — summarize the older half of a long session into a
//! single synthetic exchange so the model's effective working context shrinks
//! before "Lost in the Middle" / context-rot starts degrading output quality.
//!
//! ## Why this exists
//!
//! Research consistently shows Claude's accuracy starts dropping around
//! 20-40% context fill, well before the hard 200k cap. Long OPC sessions
//! routinely cross this boundary. Manual `/clear` works but throws away
//! valuable history; this module keeps the gist while shedding the bulk.
//!
//! ## Safety rules
//!
//! Slicing messages in the middle of a tool_use → tool_result sequence
//! orphans the tool_use block, which the API rejects (HTTP 400). So we
//! only cut at a "clean boundary":
//!
//!   - The cut-point's last kept message is an Assistant text-only reply
//!     (no `ToolUse` blocks)
//!   - The cut-point's next message is a User message
//!
//! If no clean boundary lets us drop ≥ 4 messages while keeping ≥ 6 recent
//! ones, the function returns `Ok(None)` and the caller skips compaction.
//!
//! ## Output
//!
//! On success the session's `messages` field is replaced with:
//!
//!   1. One User message: `## Previous conversation summary\n\n{model output}`
//!   2. One Assistant message: `了解。我会基于此摘要继续。`
//!   3. … all preserved recent messages …
//!
//! `session.record_compaction` is called and `save_to_path` persists the
//! new snapshot atomically.

use api::{
    InputContentBlock, InputMessage, MessageRequest, OutputContentBlock, ProviderClient,
    ToolChoice,
};
use runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
use serde::{Deserialize, Serialize};

use crate::config::{normalize_model, DesktopConfig};

/// Minimum total messages before compaction is even considered.
const MIN_MESSAGES: usize = 16;

/// Always preserve at least this many most-recent messages.
const KEEP_RECENT: usize = 8;

/// Always drop at least this many messages — anything smaller isn't worth
/// the round-trip / token cost.
const MIN_DROPPED: usize = 4;

const COMPACTION_SYSTEM_PROMPT: &str = "\
你是 Session Compaction Agent。你的任务是把下面的多轮对话浓缩成一段简洁的摘要，\
让另一个 AI agent 读了摘要后能继续完成工作。

## 必须包含

1. 用户的整体目标 / 当前任务
2. 已经做出的关键决策（如：用 X 而不是 Y，原因）
3. 已经完成的具体动作（哪些文件被改、哪些 sub-agent 被派出、哪些结果已经收回）
4. 仍在进行或卡住的工作
5. 用户明确表达的偏好和约束

## 不要包含

- 已经被推翻的早期方案
- 工具调用的原始 JSON / 长输出
- 思考过程中的犹豫和探索（保留结论即可）

## 输出格式

直接输出 Markdown 摘要，不要任何前缀或代码块标记，不超过 400 字。
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionReport {
    /// How many original messages were folded into the summary.
    pub dropped_message_count: usize,
    /// How many messages remain after compaction (summary pair + kept recent).
    pub kept_message_count: usize,
    /// The summary text (for UI display).
    pub summary: String,
}

/// Drive a full compaction pass on `session`. Returns `Ok(None)` when no
/// safe cut-point exists or the session is too short to bother.
pub fn compact_session(
    session: &mut Session,
    config: &DesktopConfig,
) -> Result<Option<CompactionReport>, String> {
    if session.messages.len() < MIN_MESSAGES {
        return Ok(None);
    }

    let Some(cut) = find_safe_cut(&session.messages) else {
        return Ok(None);
    };
    let dropped = cut + 1;
    let kept_recent = session.messages.len() - dropped;
    if dropped < MIN_DROPPED || kept_recent < KEEP_RECENT {
        return Ok(None);
    }

    // Build a flattened transcript of the to-be-dropped section.
    let transcript = render_transcript(&session.messages[..=cut]);

    // Ask the model to summarize. We re-use the configured provider but
    // disable thinking mode and tools — this is a single one-shot call.
    let summary = call_summarizer(&transcript, config)?;
    if summary.trim().is_empty() {
        return Err("compaction model returned empty summary".to_string());
    }

    // Build replacement: summary user-message + ack assistant-message.
    let summary_msg = ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: format!("## Previous conversation summary\n\n{summary}"),
        }],
        usage: None,
    };
    let ack_msg = ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![ContentBlock::Text {
            text: "了解。我会基于上方摘要继续后续工作。".to_string(),
        }],
        usage: None,
    };

    let mut new_messages = Vec::with_capacity(2 + kept_recent);
    new_messages.push(summary_msg);
    new_messages.push(ack_msg);
    new_messages.extend(session.messages.drain(dropped..));

    session.messages = new_messages;
    session.record_compaction(summary.clone(), dropped);

    // Persist atomically. Persistence path may be unset in tests.
    if let Some(path) = session.persistence_path().map(std::path::Path::to_path_buf) {
        session.save_to_path(&path).map_err(|e| e.to_string())?;
    }

    Ok(Some(CompactionReport {
        dropped_message_count: dropped,
        kept_message_count: session.messages.len(),
        summary,
    }))
}

/// Walk backwards from the second-to-last message looking for an Assistant
/// reply with no `ToolUse` blocks whose successor is a User message. Return
/// the index of that Assistant message (the inclusive cut-point), or `None`.
fn find_safe_cut(messages: &[ConversationMessage]) -> Option<usize> {
    if messages.len() <= KEEP_RECENT {
        return None;
    }
    let last_eligible = messages.len() - KEEP_RECENT;
    // Walk from `last_eligible - 1` down to 0 so we drop as much as possible.
    for i in (0..last_eligible).rev() {
        let m = &messages[i];
        if !matches!(m.role, MessageRole::Assistant) {
            continue;
        }
        let has_tool_use = m
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        if has_tool_use {
            continue;
        }
        let next = messages.get(i + 1)?;
        if matches!(next.role, MessageRole::User) {
            return Some(i);
        }
    }
    None
}

fn render_transcript(messages: &[ConversationMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            MessageRole::User => "USER",
            MessageRole::Assistant => "ASSISTANT",
            MessageRole::System => "SYSTEM",
            MessageRole::Tool => "TOOL",
        };
        out.push_str(&format!("--- {role} ---\n"));
        for b in &m.blocks {
            match b {
                ContentBlock::Text { text } => {
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let trimmed = truncate(input, 300);
                    out.push_str(&format!("[tool_use: {name}] input={trimmed}\n"));
                }
                ContentBlock::ToolResult {
                    output, is_error, ..
                } => {
                    let kind = if *is_error { "tool_error" } else { "tool_result" };
                    let trimmed = truncate(output, 600);
                    out.push_str(&format!("[{kind}] {trimmed}\n"));
                }
            }
        }
        out.push('\n');
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

fn call_summarizer(transcript: &str, config: &DesktopConfig) -> Result<String, String> {
    let model = normalize_model(&config.model);
    let request = MessageRequest {
        model: model.clone(),
        max_tokens: 1024,
        messages: vec![InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text {
                text: format!(
                    "请按系统提示把下面的对话浓缩成摘要：\n\n{transcript}"
                ),
            }],
            reasoning_content: None,
        }],
        system: Some(COMPACTION_SYSTEM_PROMPT.to_string()),
        tools: None,
        tool_choice: None as Option<ToolChoice>,
        stream: false,
        thinking_mode: Some(false),
        ..Default::default()
    };
    let provider = ProviderClient::from_model(&model).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let response = rt.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(120),
            provider.send_message(&request),
        )
        .await
        .map_err(|_| "compaction summarizer timed out (120s)".to_string())?
        .map_err(|e| e.to_string())
    })?;

    let text = response
        .content
        .iter()
        .filter_map(|b| match b {
            OutputContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{ContentBlock, ConversationMessage, MessageRole};

    fn user(text: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            usage: None,
        }
    }
    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            usage: None,
        }
    }
    fn assistant_tool(name: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "x".to_string(),
                name: name.to_string(),
                input: "{}".to_string(),
            }],
            usage: None,
        }
    }
    fn tool_result(out: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "x".to_string(),
                tool_name: "bash".to_string(),
                output: out.to_string(),
                is_error: false,
            }],
            usage: None,
        }
    }

    #[test]
    fn find_safe_cut_picks_latest_clean_boundary() {
        // The algorithm walks backwards from the eligibility ceiling
        // (len - KEEP_RECENT) and returns the FIRST match it sees, which
        // is the LATEST valid cut. That maximizes how much we drop.
        let mut msgs = vec![
            user("hi"),
            assistant_tool("bash"),
            tool_result("ok"),
            assistant_text("done step 1"),
            user("next"),
        ];
        for i in 0..8 {
            msgs.push(assistant_text(&format!("a{i}")));
            msgs.push(user(&format!("u{i}")));
        }
        let cut = find_safe_cut(&msgs).expect("should find a cut");
        // Cut must be < len - KEEP_RECENT (=8) and point at assistant_text
        // followed by user.
        let keep_recent = 8;
        assert!(cut < msgs.len() - keep_recent);
        assert!(matches!(msgs[cut].role, MessageRole::Assistant));
        assert!(matches!(msgs[cut + 1].role, MessageRole::User));
    }

    #[test]
    fn find_safe_cut_refuses_when_mid_tool_sequence() {
        // Only assistant-tool messages in the eligible window — no clean cut.
        let mut msgs = Vec::new();
        for _ in 0..20 {
            msgs.push(assistant_tool("bash"));
            msgs.push(tool_result("ok"));
        }
        // No assistant-text → user boundary exists at all.
        assert_eq!(find_safe_cut(&msgs), None);
    }

    #[test]
    fn compact_session_no_op_when_short() {
        let mut s = Session::new();
        s.messages.push(user("a"));
        s.messages.push(assistant_text("b"));
        let cfg = DesktopConfig::default();
        let result = compact_session(&mut s, &cfg).unwrap();
        assert!(result.is_none());
    }
}
