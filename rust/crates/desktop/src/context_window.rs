//! Per-model context window sizes, used by the Context Health Monitor to
//! compute a fill ratio (0.0 ~ 1.0) the UI can render as a coloured badge.
//!
//! We keep this small and pragmatic — exact published limits matter less
//! than the ratio. When in doubt we under-report (smaller window), so the
//! badge warns slightly earlier rather than later.

/// Best-effort lookup of the usable context window for a model id.
/// Falls back to 128k for unknown models (a common middle ground).
#[must_use]
pub fn context_window_tokens(model: &str) -> u32 {
    let m = model.trim().to_ascii_lowercase();

    // Anthropic Claude (200k default; some opus/sonnet variants offer 1M)
    if m.contains("1m") {
        return 1_000_000;
    }
    if m.contains("claude") || m.starts_with("opus") || m.starts_with("sonnet") || m.starts_with("haiku") {
        return 200_000;
    }

    // OpenAI / GPT family
    if m.contains("gpt-5") || m.contains("gpt-4.1") || m.contains("gpt-4o") {
        return 128_000;
    }
    if m.contains("gpt-4") {
        return 128_000;
    }
    if m.contains("gpt-3.5") {
        return 16_000;
    }

    // DeepSeek
    if m.contains("deepseek-v") || m.contains("deepseek-reasoner") {
        return 64_000;
    }
    if m.contains("deepseek") {
        return 32_000;
    }

    // xAI Grok
    if m.contains("grok") {
        return 128_000;
    }

    // Alibaba Qwen
    if m.contains("qwen") {
        return 128_000;
    }

    // Mistral / Llama
    if m.contains("mistral") || m.contains("mixtral") {
        return 32_000;
    }
    if m.contains("llama") {
        return 128_000;
    }

    128_000
}

/// Compute a fill ratio in 0.0 ~ 1.0. Saturates at 1.0 if the input
/// somehow exceeds the window (e.g. provider truncated silently).
#[must_use]
pub fn fill_ratio(input_tokens: u64, model: &str) -> f32 {
    let window = u64::from(context_window_tokens(model));
    if window == 0 {
        return 0.0;
    }
    let raw = input_tokens as f32 / window as f32;
    raw.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_default_200k() {
        assert_eq!(context_window_tokens("claude-opus-4-6"), 200_000);
        assert_eq!(context_window_tokens("sonnet-4"), 200_000);
    }

    #[test]
    fn deepseek_64k() {
        assert_eq!(context_window_tokens("deepseek-v4-flash"), 64_000);
    }

    #[test]
    fn unknown_falls_back() {
        assert_eq!(context_window_tokens("mystery-model"), 128_000);
    }

    #[test]
    fn fill_ratio_saturates() {
        assert!((fill_ratio(400_000, "claude-opus-4-6") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fill_ratio_midway() {
        let r = fill_ratio(100_000, "claude-opus-4-6");
        assert!((r - 0.5).abs() < 1e-3);
    }
}
