//! Detects when the model has fallen into a tool-call loop and short-circuits
//! further iterations.
//!
//! Why this exists: OPC's CEO occasionally gets stuck calling the same tool
//! (read_file on a manifest, agent status check, etc.) over and over.
//! `ceo_max_iterations` (default 200) is a hard cap, but the user has already
//! paid for ~199 wasted iterations before it trips. This detector intervenes
//! after 3 identical calls — small enough not to bother legitimate retries
//! (which usually vary a parameter), large enough to skip transient noise.
//!
//! Behavior on trip: returns a `ToolError` whose message instructs the model
//! to stop repeating and either re-read its context or produce a final
//! answer. The error feeds back as a tool_result, so the model sees it on
//! its next iteration and (in practice) breaks out of the loop.

use std::collections::VecDeque;

/// Number of consecutive identical calls that triggers the detector.
/// 3 = lenient (one "retry" is fine; two starts to look like a loop;
/// three is conclusive). Configurable via `CLAWD_LOOP_DETECTOR_THRESHOLD`.
const DEFAULT_THRESHOLD: usize = 3;

/// Ring-buffer of recent (tool_name, input_hash) pairs.
pub struct LoopDetector {
    recent: VecDeque<(String, u64)>,
    threshold: usize,
}

impl LoopDetector {
    pub fn new() -> Self {
        let threshold = std::env::var("CLAWD_LOOP_DETECTOR_THRESHOLD")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n >= 2)
            .unwrap_or(DEFAULT_THRESHOLD);
        Self {
            recent: VecDeque::with_capacity(threshold),
            threshold,
        }
    }

    /// Reset the ring buffer. Call this at the start of each conversation turn
    /// so that identical calls made in *different* turns don't cross-accumulate
    /// and trigger a false-positive. The detector is only meant to catch loops
    /// *within* a single turn's tool-use rounds.
    pub fn reset(&mut self) {
        self.recent.clear();
    }

    /// Record a tool invocation. Returns `Some(message)` when the most recent
    /// `threshold` calls all match — meaning the caller should short-circuit
    /// with a tool error to break the loop.
    pub fn record(&mut self, tool_name: &str, input: &str) -> Option<String> {
        let hash = stable_hash(input);
        self.recent.push_back((tool_name.to_string(), hash));
        while self.recent.len() > self.threshold {
            self.recent.pop_front();
        }

        if self.recent.len() < self.threshold {
            return None;
        }
        let first = self.recent.front()?;
        if self.recent.iter().all(|(n, h)| n == &first.0 && *h == first.1) {
            // Wipe so the same loop doesn't re-trigger on the very next call
            // — the model needs at least one "fresh" iteration to recover.
            self.recent.clear();
            Some(format!(
                "⚠️ 检测到工具循环：你已经连续 {} 次以相同参数调用 `{}`。\n\
                 请停止重复，重新审视上下文，并：\n\
                 1) 如果信息已足够，直接给用户最终答复；\n\
                 2) 如果需要更多信息，换一个不同的工具或参数；\n\
                 3) 如果在等待异步结果，告诉用户你在等什么。\n\
                 — Loop Detector (clawd)",
                self.threshold, tool_name
            ))
        } else {
            None
        }
    }
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// FNV-1a 64-bit hash. Cheap and deterministic — we only need equality
/// detection, not cryptographic strength.
fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_after_three_identical_calls() {
        let mut d = LoopDetector::new();
        assert!(d.record("read_file", r#"{"path":"a"}"#).is_none());
        assert!(d.record("read_file", r#"{"path":"a"}"#).is_none());
        let msg = d.record("read_file", r#"{"path":"a"}"#);
        assert!(msg.is_some(), "should trip on third identical call");
        assert!(msg.unwrap().contains("read_file"));
    }

    #[test]
    fn different_args_do_not_trip() {
        let mut d = LoopDetector::new();
        d.record("read_file", r#"{"path":"a"}"#);
        d.record("read_file", r#"{"path":"b"}"#);
        assert!(d.record("read_file", r#"{"path":"c"}"#).is_none());
    }

    #[test]
    fn different_tools_do_not_trip() {
        let mut d = LoopDetector::new();
        d.record("read_file", r#"{"path":"a"}"#);
        d.record("bash", r#"{"command":"ls"}"#);
        assert!(d.record("read_file", r#"{"path":"a"}"#).is_none());
    }

    #[test]
    fn resets_after_tripping() {
        let mut d = LoopDetector::new();
        d.record("x", "1");
        d.record("x", "1");
        assert!(d.record("x", "1").is_some());
        // Next two identical calls should not immediately re-trip;
        // the buffer was cleared.
        assert!(d.record("x", "1").is_none());
        assert!(d.record("x", "1").is_none());
    }
}
