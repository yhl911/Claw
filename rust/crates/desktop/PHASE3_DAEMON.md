# Phase 3 — Daemon process for true 7×24 long tasks

## Goal

Decouple long-running tasks from the desktop app lifetime so a user can:

- Start a long task in the app
- Close the app (Cmd+Q) or even reboot the machine
- Come back hours later and find the task either finished or progressing

Today (after Phase 2), the runner thread dies when the app exits. The
task is marked `Interrupted` on next launch and the user clicks "▶ 续跑"
to continue. Phase 3 removes the manual step by keeping the runner alive
across app sessions.

## Architecture

```
┌────────── desktop app (transient, foreground) ──────────┐
│                                                          │
│  Tauri UI, settings, sessions, conversational worker     │
│                                                          │
│      Tauri commands:                                     │
│        start_long_task / list / cancel / get_state       │
│                                                          │
│              │ IPC (unix domain socket)                  │
│              ▼                                            │
│  daemon_client::send_command()                           │
└──────────────┬───────────────────────────────────────────┘
               │
               │ /tmp/opc-desktop-daemon.sock
               │
┌──────────────▼───────────────────────────────────────────┐
│             opc-daemon (persistent, background)          │
│                                                          │
│  Spawned at desktop startup (or by launchd/systemd)      │
│  Survives desktop app close + system sleep/wake          │
│                                                          │
│  Components:                                             │
│  - Task scheduler: runs queued long tasks                │
│  - Per-task runner threads (same logic as long_runner.rs)│
│  - IPC server: handles commands from desktop             │
│  - Crash watchdog: respawns runner threads on panic      │
│                                                          │
│  Shares files with desktop:                              │
│  - ~/.../long_tasks/{id}/spec.json + state.json + ...    │
│  - ~/.../settings.json (reads at startup)                │
│  - ~/.../memory/*.md                                     │
└──────────────────────────────────────────────────────────┘
```

## Components to build

### 1. New crate `opc-daemon` (binary)

```
rust/crates/opc-daemon/
├── Cargo.toml
└── src/
    ├── main.rs        — argument parsing, IPC server start, signal handling
    ├── ipc.rs         — line-delimited JSON command protocol over unix socket
    ├── scheduler.rs   — task queue, max-concurrent enforcement
    └── lib.rs         — shared core (could import long_runner from desktop?
                         — better to extract common runner into runtime/ or
                         a new crate `long_task_core` and depend on it)
```

### 2. Extract `long_runner` + `long_task` into a new shared crate

Currently they live in `crates/desktop/src/`. The daemon needs the
same logic so we refactor:

```
rust/crates/long_task_core/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── runner.rs      — ex-long_runner.rs (minus desktop-specific
                         pieces like `permission::OpcApprover`)
    ├── store.rs       — ex-long_task.rs file layout helpers
    └── ipc_types.rs   — command/response JSON shapes
```

`opc-desktop` and `opc-daemon` both depend on `long_task_core`.

### 3. IPC protocol

Line-delimited JSON over unix domain socket
(`$XDG_RUNTIME_DIR/opc-daemon.sock` on Linux, `/tmp/...` on macOS).

```json
// Request
{"id": "req-001", "method": "start_task",
 "params": {"goal": "…", "model": "…"}}

{"id": "req-002", "method": "list_tasks"}

{"id": "req-003", "method": "cancel_task",
 "params": {"task_id": "lt-…"}}

// Response
{"id": "req-001", "ok": true, "result": {"task_id": "lt-…"}}

// Push notification (no id, daemon→client)
{"event": "task_changed", "task_id": "lt-…", "status": "running"}
```

### 4. Daemon lifecycle

**macOS (launchd)**: ship a `LaunchAgents/dev.clawcode.opc-daemon.plist`
that lazy-starts on socket activation. First IPC request to the socket
triggers daemon start.

**Linux (systemd user)**: `~/.config/systemd/user/opc-daemon.service`
with `Type=notify` and socket activation.

**Windows**: a hidden tray app started on login; out of scope for v1.

For desktop integration, the simplest first step is: desktop spawns the
daemon as a child process if not already running. On Cmd+Q, desktop
detaches (so daemon survives). When desktop relaunches, it reconnects to
the existing socket.

### 5. Desktop client changes

In `lib.rs`:
- Replace the per-task `std::thread::spawn(|| long_runner::run(...))`
  with `daemon_client::start_task(...)` (IPC call returns task_id).
- `list_long_tasks` reads via IPC instead of `long_task::list_all()`.
- A background tokio task subscribes to daemon pushes and emits
  `long-task-changed` events to the frontend.

### 6. Crash resilience

- Daemon's per-task runner threads are wrapped in `catch_unwind` (same
  as Phase 1).
- Daemon main loop is wrapped in a supervisor: if a runner thread
  panics, daemon respawns one for that task (continuing from existing
  session jsonl). State machine: `Running` → `Crashed` → auto-resume → 
  `Running`. After N crashes within M minutes, give up and mark Failed.
- Heartbeat is still useful: if daemon process itself dies (OOM, kill -9),
  desktop's startup reap_interrupted sweep catches it.

## Risks / open questions

1. **Permissions inheritance** — daemon needs to launch sub-agent threads
   that read user files. Make sure macOS doesn't prompt for "Files & 
   Folders access" on every restart.
2. **Single-user vs multi-user** — assume single user per machine. Lock 
   the socket file with file lock to prevent two daemons.
3. **Hot-reload after Settings change** — daemon caches config? If user 
   changes API key mid-task, do we restart the task or hot-swap creds?
   Probably hot-swap via env var update.
4. **Resource limits** — should daemon expose `nice`/cgroups settings so
   user can cap CPU? Defer.

## Estimated effort

| Step | Effort |
|------|--------|
| Extract `long_task_core` crate | 0.5 day |
| `opc-daemon` binary skeleton + IPC | 1 day |
| Migrate desktop calls to IPC client | 0.5 day |
| launchd/systemd installation logic | 0.5 day |
| Crash supervisor + auto-resume | 1 day |
| Testing (long-task survival across app close) | 0.5 day |
| **Total** | **~4 days** |

Phase 3 is its own focused PR.
