#![allow(
    clippy::needless_pass_by_value,
    clippy::manual_let_else,
    clippy::uninlined_format_args,
    clippy::unnecessary_debug_formatting
)]

//! `opc-daemon` — headless background process that owns long-running OPC tasks.
//!
//! Why a separate process: the desktop app's worker thread dies when the
//! user closes the window. The daemon outlives the desktop so a task
//! started at noon can still be running when the user wakes the laptop
//! at 9pm. Both processes share the same on-disk task store
//! (`~/Library/Application Support/opc-desktop/long_tasks/<id>/`), so
//! state survives even if the daemon itself crashes.
//!
//! ## IPC
//!
//! Listens on `~/Library/Application Support/opc-desktop/daemon.sock`
//! (or the equivalent on Linux/Windows). Wire format: one JSON object
//! per line, terminated by `\n`. Each connection is a request/response
//! pair (no streaming events — the desktop polls disk state files via
//! the existing `list_long_tasks` Tauri command).
//!
//! Commands:
//! - `{"method":"ping"}` → `{"ok":true,"result":"pong"}`
//! - `{"method":"start_task","params":{"goal":"...","model":"..."}}`
//!   → `{"ok":true,"result":"lt-..."}`
//! - `{"method":"resume_task","params":{"task_id":"lt-..."}}`
//!   → `{"ok":true,"result":null}`
//! - `{"method":"cancel_task","params":{"task_id":"lt-..."}}`
//!   → `{"ok":true,"result":null}`
//! - `{"method":"shutdown"}` → drops all clients, exits
//!
//! ## Lifecycle
//!
//! The daemon is spawned on demand by the desktop app's first call to
//! `start_long_task` (when no socket is listening). It detaches from the
//! parent (setsid + close stdio) so it survives Cmd+Q.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use opc_desktop_lib::config::{apply_config_to_env, load_config};
use opc_desktop_lib::event_sink::null_sink;
use opc_desktop_lib::long_runner;
use opc_desktop_lib::long_task;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct Response<T: Serialize> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

type CancelRegistry = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

fn socket_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("opc-desktop").join("daemon.sock")
}

fn pid_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("opc-desktop").join("daemon.pid")
}

fn main() {
    eprintln!("[daemon] starting, pid={}", std::process::id());

    // On startup, sweep any tasks whose worker died mid-run.
    if let Err(e) = long_task::reap_interrupted() {
        eprintln!("[daemon] reap_interrupted failed: {e}");
    }

    // Write our pid so the desktop can verify we're alive.
    let pid_p = pid_path();
    if let Some(parent) = pid_p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&pid_p, std::process::id().to_string()) {
        eprintln!("[daemon] failed to write pid file: {e}");
    }

    // Unlink any stale socket from a previous run (only safe because we
    // hold the pid file as a single-instance lock — see the desktop's
    // "is daemon running" check before spawning).
    let sock_p = socket_path();
    if sock_p.exists() {
        let _ = std::fs::remove_file(&sock_p);
    }

    let listener = match UnixListener::bind(&sock_p) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[daemon] failed to bind socket {:?}: {e}", sock_p);
            std::process::exit(1);
        }
    };
    eprintln!("[daemon] listening on {sock_p:?}");

    let cancels: CancelRegistry = Arc::new(Mutex::new(HashMap::new()));
    let shutdown = Arc::new(AtomicBool::new(false));

    // Set socket non-blocking so we can periodically check `shutdown`.
    if let Err(e) = listener.set_nonblocking(true) {
        eprintln!("[daemon] set_nonblocking failed: {e}");
    }

    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _addr)) => {
                let cancels = cancels.clone();
                let shutdown = shutdown.clone();
                std::thread::Builder::new()
                    .name("daemon-client".to_string())
                    .spawn(move || handle_client(stream, cancels, shutdown))
                    .ok();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connection — sleep briefly to avoid busy loop.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                eprintln!("[daemon] accept error: {e}");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }

    eprintln!("[daemon] shutting down");
    let _ = std::fs::remove_file(&sock_p);
    let _ = std::fs::remove_file(&pid_p);
}

fn handle_client(
    mut stream: UnixStream,
    cancels: CancelRegistry,
    shutdown: Arc<AtomicBool>,
) {
    let reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[daemon] try_clone failed: {e}");
            return;
        }
    });
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[daemon] read line err: {e}");
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let resp_line = handle_request_line(&line, &cancels, &shutdown);
        if let Err(e) = stream.write_all(resp_line.as_bytes()) {
            eprintln!("[daemon] write err: {e}");
            return;
        }
        // Connections close after one round-trip; the desktop reconnects
        // for each command (cheap on unix domain sockets).
        return;
    }
}

fn handle_request_line(
    line: &str,
    cancels: &CancelRegistry,
    shutdown: &Arc<AtomicBool>,
) -> String {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return error_resp(format!("invalid request JSON: {e}")),
    };
    eprintln!("[daemon] -> {}", req.method);

    match req.method.as_str() {
        "ping" => ok_resp(serde_json::json!("pong")),
        "start_task" => handle_start_task(req.params, cancels),
        "resume_task" => handle_resume_task(req.params, cancels),
        "cancel_task" => handle_cancel_task(req.params, cancels),
        "shutdown" => {
            shutdown.store(true, Ordering::SeqCst);
            ok_resp(serde_json::json!("ok"))
        }
        other => error_resp(format!("unknown method: {other}")),
    }
}

fn handle_start_task(params: serde_json::Value, cancels: &CancelRegistry) -> String {
    #[derive(Deserialize)]
    struct P {
        goal: String,
        #[serde(default)]
        model: Option<String>,
    }
    let p: P = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return error_resp(format!("invalid params: {e}")),
    };
    let cfg = load_config();
    apply_config_to_env(&cfg);
    let model = p.model.as_deref().unwrap_or(&cfg.model).to_string();

    let spec = match long_task::create_task(&p.goal, &model) {
        Ok(s) => s,
        Err(e) => return error_resp(format!("create task: {e}")),
    };
    let task_id = spec.task_id.clone();
    spawn_runner(task_id.clone(), cfg, false, cancels.clone());
    ok_resp(serde_json::json!(task_id))
}

fn handle_resume_task(params: serde_json::Value, cancels: &CancelRegistry) -> String {
    #[derive(Deserialize)]
    struct P {
        task_id: String,
    }
    let p: P = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return error_resp(format!("invalid params: {e}")),
    };
    let cfg = load_config();
    apply_config_to_env(&cfg);
    spawn_runner(p.task_id, cfg, true, cancels.clone());
    ok_resp(serde_json::json!(null))
}

fn handle_cancel_task(params: serde_json::Value, cancels: &CancelRegistry) -> String {
    #[derive(Deserialize)]
    struct P {
        task_id: String,
    }
    let p: P = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return error_resp(format!("invalid params: {e}")),
    };
    let guard = match cancels.lock() {
        Ok(g) => g,
        Err(_) => return error_resp("cancel registry poisoned".into()),
    };
    match guard.get(&p.task_id) {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            ok_resp(serde_json::json!(null))
        }
        None => error_resp(format!(
            "task '{}' is not running on this daemon",
            p.task_id
        )),
    }
}

fn spawn_runner(
    task_id: String,
    cfg: opc_desktop_lib::config::DesktopConfig,
    resume: bool,
    cancels: CancelRegistry,
) {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut g) = cancels.lock() {
        g.insert(task_id.clone(), flag.clone());
    }
    let cancels_cleanup = cancels.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::Builder::new()
        .name(format!("long-runner-{task_id}"))
        .spawn(move || {
            // Headless: no UI sink. Desktop polls state.json on disk for
            // progress / completion.
            let result = long_runner::run(
                &task_id_for_thread,
                &cfg,
                flag,
                null_sink(),
                resume,
            );
            if let Ok(mut g) = cancels_cleanup.lock() {
                g.remove(&task_id_for_thread);
            }
            if let Err(e) = result {
                eprintln!("[daemon] task '{task_id_for_thread}' ended with error: {e}");
            } else {
                eprintln!("[daemon] task '{task_id_for_thread}' completed");
            }
        })
        .ok();
}

fn ok_resp<T: Serialize>(result: T) -> String {
    let r = Response {
        ok: true,
        result: Some(result),
        error: None,
    };
    serialize_response(&r)
}

fn error_resp(message: String) -> String {
    let r: Response<serde_json::Value> = Response {
        ok: false,
        result: None,
        error: Some(message),
    };
    serialize_response(&r)
}

fn serialize_response<T: Serialize>(r: &Response<T>) -> String {
    let mut s = serde_json::to_string(r).unwrap_or_else(|e| {
        format!(r#"{{"ok":false,"error":"serialize: {e}"}}"#)
    });
    s.push('\n');
    s
}
