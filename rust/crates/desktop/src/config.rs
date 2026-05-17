use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::hooks::HookSpec;

/// User-defined MCP (Model Context Protocol) server. Each entry launches an
/// external stdio process whose tools become callable by CEO and sub-agents.
///
/// Persisted in the desktop config; wiring to the runtime's MCP server
/// manager lives in `state.rs::DesktopState::build` (TODO — currently the
/// schema is collected from the UI but tools are not yet registered. See
/// `crates/runtime/src/mcp_stdio.rs::McpServerManager` for the integration
/// surface.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Disabled servers are kept in the list but not launched.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub opc_mode: bool,
    /// DeepSeek thinking mode (only applies to thinking-capable models like
    /// `deepseek-v4-flash`). Default false: thinking disabled — required for
    /// OPC CEO tool-calling stability since multi-turn + tool_use + thinking
    /// requires reasoning_content passback.
    #[serde(default)]
    pub thinking_mode: bool,
    /// Run a dreaming consolidation pass automatically when the user clears
    /// the session. Default false (must opt in to spend tokens silently).
    #[serde(default)]
    pub auto_dream: bool,
    /// When auto_dream fires, "review" emits a `dream-pending` event for
    /// the UI to show a review modal; "apply" writes memory immediately.
    /// Defaults to "review" — never silently mutate user memory.
    #[serde(default = "default_dream_mode")]
    pub auto_dream_mode: String,
    /// External MCP servers configured by the user.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerSpec>,
    /// User-defined hooks (shell commands fired on event boundaries —
    /// turn complete, session clear, long-task finish). See
    /// [`crate::hooks`] for the supported event names and payload shape.
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
    /// Permission policy mode for tool execution.
    /// - `"danger-full-access"` (default): no escalation prompts, all tools
    ///   run freely. Right for a local desktop app the user installed and
    ///   trusts.
    /// - `"workspace-write"`: tools requiring full access (bash, network
    ///   tools) are denied unless explicitly whitelisted. Stricter, for
    ///   users who want a brake.
    /// - `"read-only"`: write/exec tools all blocked. Useful for demos.
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// Hard daily USD cap. When the accumulated cost in the last 24h
    /// reaches this number the worker refuses to start the next turn.
    /// `0.0` disables the cap (default).
    #[serde(default)]
    pub budget_daily_usd: f64,
    /// Hard monthly USD cap. Same behavior as daily, summed over the
    /// trailing 30 days. `0.0` disables.
    #[serde(default)]
    pub budget_monthly_usd: f64,
    /// Optional GitHub personal-access token. Lifts the API rate limit
    /// (60/hr → 5000/hr) and is used when listing or importing skills
    /// from GitHub. Empty = unauthenticated (works for public repos but
    /// slow if quota is exhausted).
    #[serde(default)]
    pub github_token: String,
    /// Auto-compact threshold. When the context fill ratio crosses this
    /// (0.0~1.0) at the end of a turn, the worker runs a compaction pass
    /// to fold older messages into a single summary. `0.0` disables.
    /// Recommended: 0.7. Models start degrading well before this but
    /// users typically tolerate that for a few turns before noticing.
    #[serde(default)]
    pub auto_compact_threshold: f32,
}

fn default_dream_mode() -> String {
    "review".to_string()
}

fn default_permission_mode() -> String {
    "danger-full-access".to_string()
}

/// Map the string field to the runtime enum. Unknown values fall back to
/// the safe full-access default — a typo in settings.json shouldn't brick
/// the desktop.
#[must_use]
pub fn parse_permission_mode(s: &str) -> runtime::PermissionMode {
    match s.trim().to_ascii_lowercase().as_str() {
        "read-only" | "readonly" => runtime::PermissionMode::ReadOnly,
        "workspace-write" | "workspace_write" | "workspace" => {
            runtime::PermissionMode::WorkspaceWrite
        }
        _ => runtime::PermissionMode::DangerFullAccess,
    }
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-6".to_string(),
            api_key: String::new(),
            base_url: String::new(),
            opc_mode: true,
            thinking_mode: false,
            auto_dream: false,
            auto_dream_mode: default_dream_mode(),
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
            github_token: String::new(),
            budget_daily_usd: 0.0,
            budget_monthly_usd: 0.0,
            permission_mode: default_permission_mode(),
            auto_compact_threshold: 0.0,
        }
    }
}

/// Strip trailing `/chat/completions` (users often paste the full endpoint URL).
/// The OpenAI-compat client appends this path itself.
pub fn normalize_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed
        .strip_suffix("/chat/completions")
        .unwrap_or(trimmed)
        .to_string()
}

/// Ensure non-Claude models are prefixed with `openai/` so the provider
/// router sends them through the OpenAI-compat path instead of Anthropic.
pub fn normalize_model(model: &str) -> String {
    let m = model.trim();
    if m.is_empty() {
        return m.to_string();
    }
    // Already has a provider prefix
    if m.contains('/') {
        return m.to_string();
    }
    // Known Anthropic model families — keep as-is
    if m.starts_with("claude-") || m.starts_with("opus") || m.starts_with("sonnet") || m.starts_with("haiku") {
        return m.to_string();
    }
    // Everything else (deepseek-*, gpt-*, qwen-*, mistral-*, etc.) needs openai/ prefix
    format!("openai/{m}")
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opc-desktop")
        .join("settings.json")
}

pub fn load_config() -> DesktopConfig {
    let path = config_path();
    let mut cfg: DesktopConfig = if let Ok(text) = std::fs::read_to_string(&path) {
        serde_json::from_str(&text).unwrap_or_default()
    } else {
        DesktopConfig {
            model: std::env::var("OPC_MODEL")
                .unwrap_or_else(|_| "claude-opus-4-6".to_string()),
            api_key: std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                .unwrap_or_default(),
            base_url: std::env::var("OPENAI_BASE_URL")
                .or_else(|_| std::env::var("ANTHROPIC_BASE_URL"))
                .unwrap_or_default(),
            opc_mode: true,
            thinking_mode: false,
            auto_dream: false,
            auto_dream_mode: default_dream_mode(),
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
            github_token: String::new(),
            budget_daily_usd: 0.0,
            budget_monthly_usd: 0.0,
            permission_mode: default_permission_mode(),
            auto_compact_threshold: 0.0,
        }
    };
    // Transparent re-hydration from the platform keyring. If a previous
    // save migrated the secret out of settings.json, the JSON field is
    // blank — fill it back from the vault so the rest of the app, the
    // worker, and the settings UI all see the secret as usual.
    if cfg.api_key.trim().is_empty() {
        if let Ok(Some(v)) = crate::vault::load("api_key") {
            cfg.api_key = v;
        }
    }
    if cfg.github_token.trim().is_empty() {
        if let Ok(Some(v)) = crate::vault::load("github_token") {
            cfg.github_token = v;
        }
    }
    cfg
}

pub fn save_config(config: &DesktopConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Migrate secrets to the keyring. On vault failure (e.g., Linux
    // without a Secret Service backend) fall through and keep the field
    // in the JSON file as a fallback — losing a saved key silently is
    // worse than leaving it in the user's app-support folder.
    let mut to_persist = config.clone();
    if !to_persist.api_key.trim().is_empty() {
        match crate::vault::store("api_key", &to_persist.api_key) {
            Ok(()) => to_persist.api_key.clear(),
            Err(e) => eprintln!("[config] vault store api_key failed: {e}; keeping in JSON"),
        }
    } else {
        // User explicitly cleared the field — drop the keyring entry too
        // so it can't resurrect itself on next load.
        let _ = crate::vault::delete("api_key");
    }
    if !to_persist.github_token.trim().is_empty() {
        match crate::vault::store("github_token", &to_persist.github_token) {
            Ok(()) => to_persist.github_token.clear(),
            Err(e) => eprintln!("[config] vault store github_token failed: {e}; keeping in JSON"),
        }
    } else {
        let _ = crate::vault::delete("github_token");
    }

    let text = serde_json::to_string_pretty(&to_persist).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Apply config to environment variables so provider clients pick them up.
/// Normalizes base URL (strips /chat/completions) and model prefix.
pub fn apply_config_to_env(config: &DesktopConfig) {
    if !config.api_key.is_empty() {
        std::env::set_var("OPENAI_API_KEY", &config.api_key);
        std::env::set_var("ANTHROPIC_API_KEY", &config.api_key);
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", &config.api_key);
    }
    if !config.base_url.is_empty() {
        let clean_url = normalize_base_url(&config.base_url);
        std::env::set_var("OPENAI_BASE_URL", &clean_url);
        std::env::set_var("ANTHROPIC_BASE_URL", &clean_url);
    }
    // Surface the GitHub token to the skills loader (which reads it from
    // env). Setting (and clearing) here keeps the env in sync with the
    // persisted setting whenever the user updates it in the UI.
    if config.github_token.trim().is_empty() {
        std::env::remove_var("OPC_GITHUB_TOKEN");
    } else {
        std::env::set_var("OPC_GITHUB_TOKEN", config.github_token.trim());
    }

    // Make sub-agents inherit the CEO's configured model. Without this,
    // sub-agents fall back to `claude-opus-4-6` and hit the user's
    // (non-Anthropic) base URL with the wrong wire format → 404.
    if !config.model.trim().is_empty() {
        let normalized = normalize_model(&config.model);
        std::env::set_var("CLAWD_DEFAULT_AGENT_MODEL", &normalized);
    }
    // Force a stable, predictable agent manifest location for the desktop
    // app. The default heuristic (`cwd.ancestors().nth(2)`) is unreliable
    // when launched outside a workspace — manifests would land somewhere
    // the right-side panel can't find them.
    if let Some(data_dir) = dirs::data_dir() {
        let app_dir = data_dir.join("opc-desktop");
        let agent_dir = app_dir.join("agents");
        let _ = std::fs::create_dir_all(&agent_dir);
        std::env::set_var("CLAWD_AGENT_STORE", &agent_dir);

        // Point the runtime's skill discovery (`CLAW_CONFIG_HOME/skills`)
        // at our app data dir so the `Skill` tool finds skills the
        // desktop manages. Skills live at `<app_dir>/skills/<name>/SKILL.md`.
        let skills_dir = app_dir.join("skills");
        let _ = std::fs::create_dir_all(&skills_dir);
        std::env::set_var("CLAW_CONFIG_HOME", &app_dir);
    }
}
