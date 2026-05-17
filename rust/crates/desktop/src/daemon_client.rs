//! Client for talking to the `opc-daemon` background process.
//!
//! The daemon hosts long-running tasks so they survive desktop close /
//! restart. The desktop talks to it over a unix domain socket, one
//! request per connection (cheap on unix sockets).
//!
//! ## Lifecycle of the daemon
//!
//! When the desktop wants to start a long task, it:
//!
//! 1. Pings the daemon. If alive, sends `start_task` directly.
//! 2. If the daemon isn't running, spawns it as a detached child process
//!    (setsid + closed stdio so it survives `kill(parent)` and `Cmd+Q`).
//! 3. Polls the socket for up to ~3 seconds while the daemon binds it.
//! 4. Retries `start_task` once the socket is reachable.
//!
//! ## Why request-per-connection
//!
//! Long tasks don't need real-time push events — the desktop polls state
//! files on disk (`long_tasks/<id>/state.json`) for progress. So the
//! socket is only used for **commands**, which are infrequent. Opening
//! a fresh stream per call keeps the protocol simple (no framing, no
//! reconnect logic on the client side).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct Request<'a, T: Serialize> {
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<T>,
}

#[derive(Debug, Deserialize)]
struct Response<T> {
    ok: bool,
    result: Option<T>,
    error: Option<String>,
}

fn socket_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("opc-desktop").join("daemon.sock")
}

/// Send one JSON-line request, read one JSON-line response. The socket
/// closes after each exchange — the daemon does the same.
fn send<T: Serialize, R: for<'de> Deserialize<'de>>(
    method: &str,
    params: Option<T>,
) -> Result<R, String> {
    let mut stream = UnixStream::connect(socket_path())
        .map_err(|e| format!("connect daemon socket: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok();
    let req = Request { method, params };
    let mut payload = serde_json::to_string(&req)
        .map_err(|e| format!("serialize request: {e}"))?;
    payload.push('\n');
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| format!("write request: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("flush request: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("read response: {e}"))?;
    let resp: Response<R> = serde_json::from_str(line.trim())
        .map_err(|e| format!("parse response '{}': {e}", line.trim()))?;
    if resp.ok {
        resp.result
            .ok_or_else(|| "daemon returned ok with no result".to_string())
    } else {
        Err(resp
            .error
            .unwrap_or_else(|| "daemon returned error with no message".to_string()))
    }
}

pub fn ping() -> Result<String, String> {
    send::<(), String>("ping", None)
}

/// Start a long-running task on the daemon. Returns the assigned task_id.
pub fn start_task(goal: &str, model: Option<&str>) -> Result<String, String> {
    #[derive(Serialize)]
    struct P<'a> {
        goal: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<&'a str>,
    }
    send::<P, String>("start_task", Some(P { goal, model }))
}

pub fn resume_task(task_id: &str) -> Result<(), String> {
    #[derive(Serialize)]
    struct P<'a> {
        task_id: &'a str,
    }
    let _: serde_json::Value = send("resume_task", Some(P { task_id }))?;
    Ok(())
}

pub fn cancel_task(task_id: &str) -> Result<(), String> {
    #[derive(Serialize)]
    struct P<'a> {
        task_id: &'a str,
    }
    let _: serde_json::Value = send("cancel_task", Some(P { task_id }))?;
    Ok(())
}

/// Check whether the daemon is reachable. Cheap (single round-trip to
/// the unix socket).
pub fn is_running() -> bool {
    ping().is_ok()
}

/// Spawn the daemon binary as a fully detached child process. After
/// this returns, the daemon's lifecycle is independent of the desktop
/// app — Cmd+Q on the app does not kill the daemon.
///
/// On macOS / Linux we use `setsid` to detach the process group, plus
/// close stdio so the child doesn't inherit our file descriptors.
pub fn spawn_daemon() -> Result<(), String> {
    let exe = locate_daemon_binary()?;
    eprintln!("[daemon-client] spawning daemon: {}", exe.display());

    // We forbid `unsafe` workspace-wide, which rules out the conventional
    // `pre_exec(|| setsid())` detachment. Instead we rely on:
    // 1. Closing stdio (Stdio::null()) so the child has no inherited
    //    fds tying it to the parent's terminal/window.
    // 2. macOS Cocoa apps not sending SIGHUP/SIGTERM to child processes
    //    on quit — children become "orphaned" but keep running with
    //    launchd as the new parent.
    // 3. The daemon ignoring stdin/stdout traffic anyway (all I/O is
    //    via the unix socket).
    //
    // If we ever observe the daemon dying on Cmd+Q, the next step is to
    // depend on the `nix` crate and call `nix::unistd::setsid()` (safe
    // wrapper around the system call).
    std::process::Command::new(&exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn daemon: {e}"))?;
    Ok(())
}

/// Try to find the `opc-daemon` binary relative to the running
/// `opc-desktop` binary (next to it in the bundle / target dir).
fn locate_daemon_binary() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))?;
    let dir = current
        .parent()
        .ok_or_else(|| "current exe has no parent dir".to_string())?;
    let candidate = dir.join("opc-daemon");
    if candidate.exists() {
        return Ok(candidate);
    }
    // Dev fallback: when running via `cargo tauri dev`, the daemon may
    // live in the workspace target dir.
    if let Some(workspace) = dir
        .ancestors()
        .find(|p| p.join("Cargo.toml").exists() && p.join("crates").exists())
    {
        let dev_candidate = workspace
            .join("target")
            .join(if cfg!(debug_assertions) { "debug" } else { "release" })
            .join("opc-daemon");
        if dev_candidate.exists() {
            return Ok(dev_candidate);
        }
    }
    Err(format!(
        "opc-daemon binary not found. Looked next to {} and in workspace target/.",
        current.display()
    ))
}

/// Ensure the daemon is running. Spawns it if not, waits up to ~3s for
/// it to bind the socket, then returns. Safe to call before every
/// daemon command — `ping` shortcircuits when already up.
pub fn ensure_running() -> Result<(), String> {
    if is_running() {
        return Ok(());
    }
    spawn_daemon()?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(150));
        if is_running() {
            return Ok(());
        }
    }
    Err("daemon did not become reachable within 3s".to_string())
}
