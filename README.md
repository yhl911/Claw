# Claw — OPC Desktop + CLI

<p align="center">
  <img src="assets/screenshots/icon.png" alt="Claw Icon" width="96" />
</p>

<p align="center">
  <strong>AI-native macOS desktop for one-person companies.</strong><br/>
  A CEO Agent that orchestrates 7 specialized sub-agents, built on Claude.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-macOS-blue?logo=apple" />
  <img src="https://img.shields.io/badge/built_with-Tauri_v2-orange?logo=rust" />
  <img src="https://img.shields.io/badge/AI-Claude_3.x_%2F_4.x-blueviolet?logo=anthropic" />
  <img src="https://img.shields.io/badge/license-MIT-green" />
</p>

---

## Overview

**Claw** combines a production-grade AI CLI (`claw`) with a native macOS desktop app — **OPC Desktop** — that puts a full one-person-company command center in a single window.

You talk to a CEO Agent. It delegates to 7 domain-specific sub-agents. All agents share a workspace, a session, and a common tool surface.

<!-- SCREENSHOT: full 3-column layout (SessionSidebar | ChatPanel | OpcAgentPanel) -->
<!-- Add screenshots/main.png after taking a screenshot of the running app -->
<!--
![OPC Desktop main view](assets/screenshots/main.png)
-->

---

## Features

### 🏢 OPC Mode — One-Person Company CEO Agent

The core innovation: a **CEO Agent** that understands your business context and routes work to specialized sub-agents.

| Sub-agent | Specialty |
|-----------|-----------|
| `opc-product` | Product strategy, roadmap, user research |
| `opc-engineering` | Architecture, code review, technical decisions |
| `opc-finance` | P&L, budgeting, financial analysis |
| `opc-marketing` | Copy, campaigns, positioning |
| `opc-sales` | Pipeline, outreach, CRM strategy |
| `opc-ops` | Ops, hiring, process, legal-adjacent |
| `opc-legal` | Contracts, compliance, risk |

The CEO routes tasks automatically — just describe what you need. Sub-agent panels appear live as delegation happens.

### 🧠 Intelligent Context Management

- **Context fill bar** — real-time visualization of how full the context window is
- **Auto-compaction** — when context nears capacity, important content is summarized and the window is cleared automatically; no lost history
- **Decision Anchors** — key decisions are extracted and re-injected as compact anchor blocks, so the CEO never forgets critical choices even after compaction
- **Dream pass** — background memory consolidation distills sessions into durable insights

### 🔄 Loop Detection

A FNV-1a hash ring buffer detects when the model is stuck in a repetitive pattern. When a loop is detected, the current turn is gracefully interrupted before wasting tokens.

### 🔐 Secure Key Storage

API keys are stored in the **macOS Keychain** (not in files, not in env vars). Keys are read at runtime and never written to disk in plaintext.

### 📡 Multi-Provider Support

| Prefix / env | Provider |
|---|---|
| *(default)* | Anthropic (Claude) |
| `openai/` model prefix | OpenAI-compatible |
| `grok-*` | xAI / Grok |
| `qwen-*` | Alibaba DashScope |
| `OPENAI_BASE_URL` | Any OpenAI-compatible endpoint |

### 💬 Native Chat UX

- Real-time SSE streaming with token-by-token rendering
- Markdown + code block rendering
- Session sidebar with history
- Cancel in-flight requests
- Confirm dialog before clearing sessions

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    OPC Desktop (Tauri v2)                │
│  ┌──────────────┬────────────────┬─────────────────────┐ │
│  │ SessionSide  │   ChatPanel    │   OpcAgentPanel     │ │
│  │ bar          │                │                     │ │
│  │  • Sessions  │  CEO Agent     │  • Active agents    │ │
│  │  • History   │  conversation  │  • Sub-agent status │ │
│  │              │                │  • Delegation log   │ │
│  └──────────────┴────────────────┴─────────────────────┘ │
│                          │                               │
│              Tauri IPC (Rust backend)                    │
│                          │                               │
│  ┌───────────────────────────────────────────────────┐   │
│  │              WorkerMsg Channel                    │   │
│  │   ConversationRuntime + Session + ToolExecutor    │   │
│  └───────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
                           │
              ┌────────────┴─────────────┐
              │      Claw Crates         │
              │  api/ runtime/ tools/    │
              │  commands/ plugins/      │
              └──────────────────────────┘
```

### Key crates

| Crate | Role |
|---|---|
| `api` | HTTP client, SSE streaming, provider abstraction |
| `runtime` | `ConversationRuntime`, session persistence, MCP, hooks |
| `tools` | All tool implementations + `GlobalToolRegistry` |
| `commands` | Slash commands, parse/validate, renderers |
| `rusty-claude-cli` | CLI entry point, REPL loop |
| `desktop` | Tauri backend: state, workers, dream, loop detection |

---

## Getting Started

### Prerequisites

- macOS 13+ (Apple Silicon or Intel)
- [Rust](https://rustup.rs/) + `cargo`
- [Node.js](https://nodejs.org/) 18+ and [pnpm](https://pnpm.io/)
- [Tauri CLI](https://tauri.app/start/prerequisites/): `cargo install tauri-cli`
- An **Anthropic API key** (`sk-ant-...`)

### Development (hot reload)

```bash
# 1. Install frontend deps
cd rust/crates/desktop/ui
pnpm install

# 2. Start desktop app in dev mode (from repo root)
cd rust
cargo tauri dev --config crates/desktop/tauri.conf.json
```

The app will open with hot reload on frontend changes.

### Build DMG installer

```bash
cd rust
cargo tauri build --config crates/desktop/tauri.conf.json
# Output: rust/crates/desktop/target/release/bundle/dmg/OPC Desktop_0.1.0_aarch64.dmg
```

### CLI only

```bash
cd rust
cargo build --workspace
export ANTHROPIC_API_KEY="sk-ant-..."
./target/debug/claw doctor   # health check
./target/debug/claw          # interactive REPL
./target/debug/claw --opc    # CEO Agent mode (CLI)
```

---

## Configuration

| File | Purpose |
|---|---|
| `.claw.json` | Project-level defaults (model, permissions) |
| `.claw/settings.local.json` | Machine-local overrides (gitignored) |
| macOS Keychain | API key storage (set via Settings modal in desktop app) |

### Environment variables

```bash
ANTHROPIC_API_KEY=sk-ant-...       # Anthropic auth
ANTHROPIC_BASE_URL=...             # Optional proxy
OPENAI_API_KEY=sk-...              # OpenAI-compat providers
OPENAI_BASE_URL=https://...        # OpenAI-compat endpoint
```

---

## CLI Quick Reference

```bash
claw                    # Interactive REPL
claw --opc              # CEO Agent mode
claw prompt "..."       # One-shot prompt
claw doctor             # Health check
claw session list       # List sessions
```

**Slash commands (in REPL or desktop):**

```
/clear              Clear session (with confirm dialog)
/compact            Manual compaction
/model <name>       Switch model
/opc-agents         List active OPC sub-agents
/help               Show all commands
```

---

## Roadmap

- [ ] iOS / iPad companion app
- [ ] Team mode: shared sub-agent sessions
- [ ] Plugin marketplace for custom sub-agents
- [ ] Voice input for CEO Agent
- [ ] Export conversations to Notion / Confluence
- [ ] Local model support (Ollama)

---

## Contributing

PRs welcome. Run `scripts/fmt.sh` before committing. CI enforces `cargo clippy -D warnings`.

```bash
# Test
cd rust && cargo test --workspace

# Lint
cargo clippy --workspace --all-targets -- -D warnings

# Format
../scripts/fmt.sh
```

---

## License

MIT — see [LICENSE](LICENSE).

---

<p align="center">
  Built with ☕ and Claude.
</p>
