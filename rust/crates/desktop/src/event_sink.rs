//! Abstract event sink for code that needs to run in both Tauri context
//! (desktop app) and headless context (daemon binary).
//!
//! - `TauriSink` wraps a `tauri::AppHandle` and forwards events to the
//!   webview via Tauri's event bus.
//! - `NullSink` drops all events — used by the daemon's long-task runner
//!   where there is no UI to notify. The desktop polls state files on
//!   disk to learn about progress instead.

use std::sync::Arc;

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// Type alias for the boxed sink everywhere downstream code holds it.
pub type Sink = Arc<dyn EventSink>;

/// Sink that drops all events. Useful in the daemon (no UI to notify),
/// in tests, and as a safe default.
#[allow(dead_code)] // consumed by `opc-daemon` binary crate via lib re-export
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {
        // intentional no-op
    }
}

/// Sink that forwards events through Tauri's event bus to the frontend.
pub struct TauriSink {
    handle: tauri::AppHandle,
}

impl TauriSink {
    #[must_use]
    pub fn new(handle: tauri::AppHandle) -> Self {
        Self { handle }
    }
}

impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.handle.emit(event, payload);
    }
}

#[must_use]
#[allow(dead_code)] // consumed by `opc-daemon` binary crate
pub fn null_sink() -> Sink {
    Arc::new(NullSink)
}

#[must_use]
pub fn tauri_sink(handle: tauri::AppHandle) -> Sink {
    Arc::new(TauriSink::new(handle))
}
