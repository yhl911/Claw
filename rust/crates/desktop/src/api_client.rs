use api::{
    ContentBlockDelta, ImageSource, InputContentBlock, InputMessage, MessageRequest,
    OutputContentBlock, ProviderClient, StreamEvent, ToolChoice, ToolDefinition,
    ToolResultContentBlock,
};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, MessageRole,
    RuntimeError, TokenUsage,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::event_sink::Sink;

/// Sentinel marker prefix on `RuntimeError` strings produced by user-cancellation.
/// The worker can detect this to emit a friendlier message.
pub const CANCELLED_MARKER: &str = "__OPC_CANCELLED__";

/// Tauri event name for streaming partial assistant output. The frontend
/// subscribes to this and progressively renders text deltas + tool calls.
pub const TURN_STREAM_EVENT: &str = "turn-stream";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TurnStreamPayload {
    /// Streaming text from the assistant (one delta per SSE chunk).
    TextDelta { text: String },
    /// Assistant invoked a tool. The frontend renders an inline card.
    ToolStart {
        tool_use_id: String,
        tool_name: String,
        /// Best-effort short summary of the input (first 120 chars of JSON).
        input_preview: String,
    },
    /// New iteration of the run_turn loop has started (post-tool-result).
    Iteration { n: usize },
}

pub struct DesktopApiClient {
    pub client: ProviderClient,
    pub model: String,
    pub enable_tools: bool,
    pub tool_specs: Vec<ToolDefinition>,
    rt: tokio::runtime::Runtime,
    /// Accumulated `reasoning_content` per assistant turn (for `DeepSeek` thinking models).
    /// Index 0 = first assistant turn, index 1 = second, etc.
    reasoning_history: Vec<String>,
    /// `DeepSeek` thinking-mode override. None = backend default.
    thinking_mode: Option<bool>,
    /// Shared cancellation flag. The Tauri `cancel_turn` command flips this
    /// to true; the stream loop checks it between events and bails with a
    /// `CANCELLED_MARKER` error so the worker can emit a clean cancel state.
    cancel_flag: Arc<AtomicBool>,
    /// Event sink for emitting streaming events. In desktop context this
    /// is a `TauriSink` that pushes to the webview; in the daemon this
    /// is `NullSink` (no UI to notify — desktop polls state files).
    sink: Sink,
    /// Iteration counter — bumped each time `stream()` is called within a turn.
    iteration: usize,
}

impl DesktopApiClient {
    pub fn new(
        client: ProviderClient,
        model: String,
        enable_tools: bool,
        tool_specs: Vec<ToolDefinition>,
        thinking_mode_enabled: bool,
        cancel_flag: Arc<AtomicBool>,
        sink: Sink,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Only override for DeepSeek thinking-capable models. For other models
        // (Claude, GPT, etc.) leave as None so backend default applies.
        let thinking_mode = is_deepseek_thinking_model(&model).then_some(thinking_mode_enabled);
        Ok(Self {
            client,
            model,
            enable_tools,
            tool_specs,
            rt: tokio::runtime::Runtime::new()?,
            reasoning_history: Vec::new(),
            thinking_mode,
            cancel_flag,
            sink,
            iteration: 0,
        })
    }

    fn emit_stream(&self, payload: TurnStreamPayload) {
        if let Ok(v) = serde_json::to_value(&payload) {
            self.sink.emit(TURN_STREAM_EVENT, v);
        }
    }
}

/// Truncate at character boundary (NOT byte) and append an ellipsis when
/// the string is longer than `max_chars`. Slicing by byte index panics on
/// multi-byte characters like Chinese.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn is_deepseek_thinking_model(model: &str) -> bool {
    // strip openai/ prefix
    let bare = model.strip_prefix("openai/").unwrap_or(model);
    bare.starts_with("deepseek-v") || bare.starts_with("deepseek-reasoner")
}

impl ApiClient for DesktopApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let tools: Option<Vec<ToolDefinition>> = self
            .enable_tools
            .then(|| self.tool_specs.clone());
        let messages = convert_messages_with_reasoning(&request.messages, &self.reasoning_history);
        let message_request = MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages,
            system: (!request.system_prompt.is_empty())
                .then(|| request.system_prompt.join("\n\n")),
            tools,
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
            thinking_mode: self.thinking_mode,
            ..Default::default()
        };

        eprintln!("[api] stream() called, model={:?}, msgs={}", self.model, message_request.messages.len());

        // Reset cancel flag at the start of each turn so the previous turn's
        // cancel state cannot leak into this one.
        self.cancel_flag.store(false, Ordering::SeqCst);
        let cancel_flag = self.cancel_flag.clone();
        let sink = self.sink.clone();

        // Bump iteration counter and emit a frontend hint that we're about
        // to fetch the next assistant turn (e.g., after a tool result).
        self.iteration += 1;
        self.emit_stream(TurnStreamPayload::Iteration { n: self.iteration });

        let sink_for_retry = sink.clone();
        let result = self.rt.block_on(async {
            // Connect with exponential-backoff retry on transient errors
            // (network blips, 429, 5xx, timeouts). Permanent errors (400/401/
            // 403) bail immediately. The cancel flag is checked between
            // attempts so the user can interrupt during backoff sleep.
            const MAX_ATTEMPTS: u32 = 4;
            let mut attempt: u32 = 0;
            let connect = loop {
                if cancel_flag.load(Ordering::SeqCst) {
                    return Err(RuntimeError::new(CANCELLED_MARKER));
                }
                eprintln!("[api] connecting to API (attempt {})...", attempt + 1);
                let raw = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    self.client.stream_message(&message_request),
                )
                .await;
                match raw {
                    Ok(Ok(stream)) => break stream,
                    Ok(Err(err)) => {
                        let msg = err.to_string();
                        let lower = msg.to_lowercase();
                        let transient = lower.contains("429")
                            || lower.contains("500")
                            || lower.contains("502")
                            || lower.contains("503")
                            || lower.contains("504")
                            || lower.contains("timeout")
                            || lower.contains("network")
                            || lower.contains("connection reset")
                            || lower.contains("connection refused")
                            || lower.contains("temporarily")
                            || lower.contains("retryable");
                        attempt += 1;
                        if !transient || attempt >= MAX_ATTEMPTS {
                            eprintln!("[api] giving up after attempt {attempt}: {msg}");
                            return Err(RuntimeError::new(msg));
                        }
                        let delay = std::time::Duration::from_secs(1u64 << attempt);
                        eprintln!(
                            "[api] transient error '{}', retrying in {}s",
                            msg.lines().next().unwrap_or(""),
                            delay.as_secs()
                        );
                        // Tell UI we're retrying so user isn't confused by long pause
                        let retry_evt = serde_json::to_value(&TurnStreamPayload::Iteration {
                            n: attempt as usize,
                        })
                        .unwrap_or_default();
                        sink_for_retry.emit(TURN_STREAM_EVENT, retry_evt);
                        // Sleep but check cancel every 250ms
                        let mut remaining = delay;
                        let tick = std::time::Duration::from_millis(250);
                        while remaining > std::time::Duration::ZERO {
                            if cancel_flag.load(Ordering::SeqCst) {
                                return Err(RuntimeError::new(CANCELLED_MARKER));
                            }
                            let step = std::cmp::min(remaining, tick);
                            tokio::time::sleep(step).await;
                            remaining = remaining.saturating_sub(step);
                        }
                    }
                    Err(_elapsed) => {
                        attempt += 1;
                        if attempt >= MAX_ATTEMPTS {
                            return Err(RuntimeError::new(
                                "Connection timed out (30s, 4 attempts). Check your Base URL and API key.",
                            ));
                        }
                        eprintln!("[api] connect timed out, retrying ({attempt}/{MAX_ATTEMPTS})");
                    }
                }
            };
            eprintln!("[api] connected, streaming...");

            // Early-cancel check (user clicked stop while we were connecting)
            if cancel_flag.load(Ordering::SeqCst) {
                eprintln!("[api] cancel detected after connect");
                return Err(RuntimeError::new(CANCELLED_MARKER));
            }

            let mut stream = connect;
            let mut events = Vec::new();
            let mut pending_tool: Option<(String, String, String)> = None;

            loop {
                if cancel_flag.load(Ordering::SeqCst) {
                    eprintln!("[api] cancel detected mid-stream");
                    return Err(RuntimeError::new(CANCELLED_MARKER));
                }
                let next = tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    stream.next_event(),
                )
                .await
                .map_err(|_| RuntimeError::new("Response timed out (120s). The model may be overloaded."))?
                .map_err(|e| RuntimeError::new(e.to_string()))?;

                let Some(event) = next else {
                    break;
                };

                match event {
                    StreamEvent::MessageStart(start) => {
                        for block in start.message.content {
                            handle_output_block(block, &mut events, &mut pending_tool);
                        }
                        if start.message.usage.input_tokens > 0
                            || start.message.usage.output_tokens > 0
                        {
                            events.push(AssistantEvent::Usage(TokenUsage {
                                input_tokens: start.message.usage.input_tokens,
                                output_tokens: start.message.usage.output_tokens,
                                cache_creation_input_tokens: start
                                    .message
                                    .usage
                                    .cache_creation_input_tokens,
                                cache_read_input_tokens: start
                                    .message
                                    .usage
                                    .cache_read_input_tokens,
                            }));
                        }
                    }
                    StreamEvent::ContentBlockStart(start) => {
                        handle_output_block(start.content_block, &mut events, &mut pending_tool);
                    }
                    StreamEvent::ContentBlockDelta(delta) => match delta.delta {
                        ContentBlockDelta::TextDelta { text } if !text.is_empty() => {
                            // Emit text delta to the UI in real time.
                            if let Ok(v) = serde_json::to_value(&TurnStreamPayload::TextDelta {
                                text: text.clone(),
                            }) {
                                sink.emit(TURN_STREAM_EVENT, v);
                            }
                            events.push(AssistantEvent::TextDelta(text));
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            if let Some((_, _, ref mut input_acc)) = pending_tool {
                                input_acc.push_str(&partial_json);
                            }
                        }
                        _ => {}
                    },
                    StreamEvent::ContentBlockStop(_) => {
                        if let Some((id, name, input)) = pending_tool.take() {
                            // Emit a card to the UI showing what tool the
                            // assistant just decided to call. The tool result
                            // arrives later (executed by the runtime); we
                            // don't currently emit ToolEnd from here because
                            // the SSE stream returns before tool execution.
                            let preview = truncate_chars(&input, 120);
                            if let Ok(v) = serde_json::to_value(&TurnStreamPayload::ToolStart {
                                tool_use_id: id.clone(),
                                tool_name: name.clone(),
                                input_preview: preview,
                            }) {
                                sink.emit(TURN_STREAM_EVENT, v);
                            }
                            events.push(AssistantEvent::ToolUse { id, name, input });
                        }
                    }
                    StreamEvent::MessageDelta(delta) => {
                        if delta.usage.output_tokens > 0 {
                            events.push(AssistantEvent::Usage(TokenUsage {
                                input_tokens: 0,
                                output_tokens: delta.usage.output_tokens,
                                cache_creation_input_tokens: 0,
                                cache_read_input_tokens: 0,
                            }));
                        }
                    }
                    StreamEvent::MessageStop(_) => {
                        events.push(AssistantEvent::MessageStop);
                        eprintln!("[api] MessageStop received, total events={}", events.len());
                        break;
                    }
                }
            }

            let reasoning = stream.take_reasoning_content();
            eprintln!("[api] stream complete, events={}, reasoning_len={}", events.len(), reasoning.len());
            Ok::<(Vec<AssistantEvent>, String), RuntimeError>((events, reasoning))
        });

        let (events, reasoning) = result?;
        if !reasoning.is_empty() {
            self.reasoning_history.push(reasoning);
        }
        Ok(events)
    }
}

fn handle_output_block(
    block: OutputContentBlock,
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
) {
    match block {
        OutputContentBlock::Text { text } if !text.is_empty() => {
            events.push(AssistantEvent::TextDelta(text));
        }
        OutputContentBlock::ToolUse { id, name, .. } => {
            *pending_tool = Some((id, name, String::new()));
        }
        _ => {}
    }
}

fn convert_messages_with_reasoning(
    messages: &[ConversationMessage],
    reasoning_history: &[String],
) -> Vec<InputMessage> {
    let mut assistant_idx = 0usize;
    messages
        .iter()
        .filter_map(|message| {
            let is_assistant = matches!(message.role, MessageRole::Assistant);
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content: Vec<InputContentBlock> = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => InputContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse { id, name, input } => InputContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::from_str(input)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": input })),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    },
                    ContentBlock::Image { media_type, data } => {
                        InputContentBlock::Image {
                            source: ImageSource {
                                source_type: "base64".to_string(),
                                media_type: media_type.clone(),
                                data: data.clone(),
                            },
                        }
                    }
                })
                .collect();

            let reasoning_content = if is_assistant {
                let rc = reasoning_history.get(assistant_idx).cloned();
                assistant_idx += 1;
                rc
            } else {
                None
            };

            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
                reasoning_content,
            })
        })
        .collect()
}

fn max_tokens_for_model(model: &str) -> u32 {
    api::max_tokens_for_model(model)
}

#[cfg(test)]
mod tests {
    use super::truncate_chars;

    #[test]
    fn truncates_ascii_at_char_count() {
        let s = "abcdefghij";
        assert_eq!(truncate_chars(s, 5), "abcde…");
    }

    #[test]
    fn keeps_short_strings_unchanged() {
        assert_eq!(truncate_chars("hi", 10), "hi");
    }

    #[test]
    fn truncates_multibyte_safely_no_panic() {
        // 广 is 3 bytes in UTF-8 — the original `&s[..120]` panicked here.
        let mut s = String::from(r#"{"description":"分析"#);
        // pad with chinese chars until we cross byte index 120
        while s.len() < 200 {
            s.push('广');
        }
        let out = truncate_chars(&s, 50);
        // No panic, char count is 50 + ellipsis
        assert_eq!(out.chars().count(), 51);
        assert!(out.ends_with('…'));
    }
}
