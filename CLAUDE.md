# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, test, format

All commands run from `rust/` unless noted.

```bash
# Build (debug)
cargo build --workspace

# Build (release)
cargo build --release

# Run all tests
cargo test --workspace

# Run a single test by name (substring match)
cargo test -p <crate> <test_name>
# e.g. cargo test -p tools run_opc_delegate

# Format (use the script, not cargo fmt directly)
../scripts/fmt.sh           # from rust/
scripts/fmt.sh              # from repo root

# Lint (warnings are errors in CI)
cargo clippy --workspace --all-targets -- -D warnings

# Health-check the built binary
./target/debug/claw doctor
```

> **Note:** `cargo fmt --manifest-path rust/Cargo.toml` is not the supported format command — always use `scripts/fmt.sh`.

## Crate architecture

```
rust/crates/
  api/               — HTTP client, SSE streaming, provider abstraction (Anthropic, OpenAI-compat, xAI, DashScope)
  runtime/           — ConversationRuntime, Session persistence, permissions, MCP plumbing, hooks, task/worker registries
  tools/             — All tool implementations (bash, file ops, Agent, OPC roles, etc.) + GlobalToolRegistry
  commands/          — SlashCommand enum, specs, parse/validate, agent/skill/MCP report renderers
  rusty-claude-cli/  — main.rs: CLI entry point, REPL loop, LiveCli, run_opc_repl, render, input
  plugins/           — Plugin manifest, hooks, test isolation
  compat-harness/    — Mock-parity test harness
  mock-anthropic-service/ — Local mock Anthropic server for testing
  telemetry/         — Analytics event types and sinks
```

### Key data-flow

1. **CLI entry** (`rusty-claude-cli/src/main.rs`) parses args → `CliAction` → dispatches to `run_repl` / `run_opc_repl` / one-shot handlers.
2. **LiveCli** holds a `ConversationRuntime<ProviderRuntimeClient, CliToolExecutor>` + `Session` + `system_prompt`.
3. **ConversationRuntime** (`runtime/src/conversation.rs`) drives the turn loop: stream API → collect assistant events → execute tools via `ToolExecutor` → push tool results back → repeat until no pending tool calls.
4. **Session** (`runtime/src/session.rs`) persists messages as JSONL under `.claw/sessions/<workspace-fingerprint>/`.
5. **Tools** (`tools/src/lib.rs`) — single large file: `mvp_tool_specs()` declares all tool schemas; `execute_tool()` dispatches by name. Sub-agent spawning (`execute_agent_with_spawn`) uses `std::thread::spawn` + file-based result passing.

### Provider routing

Model prefix determines provider: `openai/` → OpenAI-compat, `grok-*` → xAI, `qwen-*` → DashScope, everything else → Anthropic. Override base URL with `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL`.

### OPC (One-Person Company) mode

`claw --opc` or `claw opc` starts CEO Agent mode. CEO system prompt is in `OPC_CEO_SYSTEM_PROMPT` constant (`main.rs`). Sub-agent roles (`opc-product`, `opc-engineering`, `opc-finance`, `opc-marketing`, `opc-ops`) are handled in `tools/src/lib.rs`: `build_agent_system_prompt`, `allowed_tools_for_subagent`, `normalize_subagent_type`. `/opc-agents` (alias `/opc`) in REPL lists active OPC sub-agents by reading `.clawd-agents/*.json` manifests.

### Adding a new tool

1. Add a `ToolSpec` entry in `mvp_tool_specs()` (`tools/src/lib.rs`).
2. Add a match arm in `execute_tool()`.
3. Add to `allowed_tools_for_subagent()` for any sub-agent types that should have access.

### Adding a new slash command

1. Add variant to `SlashCommand` enum (`commands/src/lib.rs`).
2. Add `SlashCommandSpec` entry in `SLASH_COMMAND_SPECS`.
3. Add parse arm in `validate_slash_command_input`.
4. Add to the `handle_slash_command` fallthrough match (non-interactive resume path).
5. Add dispatch arm in `LiveCli::handle_repl_command` (`main.rs`).
6. Update `slash_command_specs().len()` assertion in `commands` tests.

## Configuration

- `.claw.json` — project-level defaults (model, permission mode)
- `.claw/settings.local.json` — machine-local overrides
- `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` — required auth
- `ANTHROPIC_BASE_URL` — optional proxy/local override
- `OPENAI_API_KEY` + `OPENAI_BASE_URL` — for OpenAI-compat providers

## Known pre-existing issues

`commands/src/lib.rs` has two `clippy::unnecessary_wraps` errors at lines ~2540 and ~2603 that pre-date this work. They cause `cargo clippy -D warnings` to fail for that crate but do not affect correctness.
