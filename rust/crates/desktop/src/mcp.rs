//! Desktop integration for MCP (Model Context Protocol) servers.
//!
//! Wires `runtime::McpServerManager` into the desktop's tool dispatch path
//! so that:
//! 1. User-configured stdio MCP servers (from Settings → 🧩 MCP Servers)
//!    are launched at app startup.
//! 2. Their tools are discovered and exposed to the model alongside the
//!    built-in `mvp_tool_specs()`.
//! 3. When the model calls one of those tools, dispatch goes through
//!    `McpServerManager::call_tool` instead of the global tool registry.
//!
//! Tools are namespaced by `{server_name}__{tool_name}` (using `__` as the
//! separator since most LLM tool name validators reject `/`). The mapping
//! between display name and runtime's qualified_name is held in
//! [`DesktopMcp::name_map`].

use api::ToolDefinition;
use runtime::{
    ConfigSource, McpServerConfig, McpServerManager, McpStdioServerConfig, ScopedMcpServerConfig,
};
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use crate::config::McpServerSpec;

#[derive(Clone)]
pub struct DesktopMcp {
    /// Shared handle. Worker holds one clone for tool dispatch; future
    /// status-panel code may hold more.
    pub manager: Arc<Mutex<McpServerManager>>,
    /// Tool specs ready to merge into the model's tool list.
    pub tool_specs: Vec<ToolDefinition>,
    /// Lookup: display name (sent to model) → runtime qualified_name
    /// (slash-separated). Populated only with tools that were discovered.
    pub name_map: HashMap<String, String>,
    /// Best-effort log line capturing what happened at init for surfacing
    /// in dev console / future settings UI.
    pub status: String,
    /// Tokio runtime for blocking on async manager methods. Dedicated to
    /// MCP so it doesn't fight with `DesktopApiClient`'s own runtime.
    pub rt: Arc<tokio::runtime::Runtime>,
}

/// Build the desktop MCP integration from user-configured server specs.
/// Returns `Ok(None)` when nothing is configured (the common case for
/// users who never opened the MCP settings).
pub fn init(specs: &[McpServerSpec]) -> Result<Option<DesktopMcp>, String> {
    let enabled: Vec<&McpServerSpec> = specs
        .iter()
        .filter(|s| s.enabled && !s.command.trim().is_empty() && !s.name.trim().is_empty())
        .collect();
    if enabled.is_empty() {
        return Ok(None);
    }

    let mut servers: BTreeMap<String, ScopedMcpServerConfig> = BTreeMap::new();
    for spec in enabled {
        let stdio_cfg = McpStdioServerConfig {
            command: spec.command.clone(),
            args: spec.args.clone(),
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
        };
        servers.insert(
            spec.name.clone(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(stdio_cfg),
            },
        );
    }

    let mut manager = McpServerManager::from_servers(&servers);
    let rt = Arc::new(
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("init MCP tokio runtime: {e}"))?,
    );

    // Discover tools — best-effort so a single broken server doesn't block
    // the whole app.
    let report = rt.block_on(manager.discover_tools_best_effort());

    let total = report.tools.len();
    let failed = report.failed_servers.len();
    let unsupported = report.unsupported_servers.len();
    eprintln!(
        "[mcp] discovered {total} tool(s) across {} server(s); {failed} failed, {unsupported} unsupported",
        servers.len()
    );

    let mut tool_specs = Vec::with_capacity(total);
    let mut name_map = HashMap::with_capacity(total);
    for managed in &report.tools {
        // Display name uses `__` so it survives most provider tool-name
        // validators (which often reject `/`, `:`, etc.).
        let display = format!(
            "mcp__{}__{}",
            sanitize_name_segment(&managed.server_name),
            sanitize_name_segment(&managed.raw_name)
        );
        let schema = managed
            .tool
            .input_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({"type": "object"}));
        tool_specs.push(ToolDefinition {
            name: display.clone(),
            description: managed.tool.description.clone(),
            input_schema: schema,
        });
        name_map.insert(display, managed.qualified_name.clone());
    }

    let status = format!(
        "MCP: {total} tool(s) ready · {failed} failed · {unsupported} unsupported"
    );
    if failed > 0 {
        for f in &report.failed_servers {
            eprintln!("[mcp] failed server '{}': {}", f.server_name, f.error);
        }
    }

    Ok(Some(DesktopMcp {
        manager: Arc::new(Mutex::new(manager)),
        tool_specs,
        name_map,
        status,
        rt,
    }))
}

/// Per-server status for the settings UI. Mirrors the shape we want to
/// render: a server name, the number of tools we successfully discovered,
/// and (if init failed) the error message — so users can see which of
/// their configured servers crashed and why without leaving the app.
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub tool_count: usize,
    pub tool_names: Vec<String>,
    pub error: Option<String>,
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct McpRuntimeStatus {
    pub summary: String,
    pub total_tools: usize,
    pub servers: Vec<McpServerStatus>,
}

/// Probe the user's configured MCP servers and return per-server status
/// without persisting any handle. Used by the settings UI to surface
/// which servers worked. This **does** launch each MCP process (we have
/// no other way to discover tool counts), so calling it has a real cost.
pub fn probe_status(specs: &[McpServerSpec]) -> McpRuntimeStatus {
    let enabled: Vec<&McpServerSpec> = specs
        .iter()
        .filter(|s| s.enabled && !s.command.trim().is_empty() && !s.name.trim().is_empty())
        .collect();
    if enabled.is_empty() {
        return McpRuntimeStatus {
            summary: "未配置任何启用的 MCP server".to_string(),
            ..Default::default()
        };
    }

    let mut servers_cfg: BTreeMap<String, ScopedMcpServerConfig> = BTreeMap::new();
    let mut entries: Vec<McpServerStatus> = Vec::with_capacity(enabled.len());
    for spec in &enabled {
        servers_cfg.insert(
            spec.name.clone(),
            ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: spec.command.clone(),
                    args: spec.args.clone(),
                    env: BTreeMap::new(),
                    tool_call_timeout_ms: None,
                }),
            },
        );
        entries.push(McpServerStatus {
            name: spec.name.clone(),
            tool_count: 0,
            tool_names: Vec::new(),
            error: None,
            unsupported_reason: None,
        });
    }

    let mut manager = McpServerManager::from_servers(&servers_cfg);
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            return McpRuntimeStatus {
                summary: format!("无法启动 tokio runtime: {e}"),
                ..Default::default()
            };
        }
    };
    let report = rt.block_on(manager.discover_tools_best_effort());

    // Tally tools per server.
    let mut tools_by_server: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for managed in &report.tools {
        tools_by_server
            .entry(managed.server_name.clone())
            .or_default()
            .push(managed.raw_name.clone());
    }
    for entry in &mut entries {
        if let Some(tools) = tools_by_server.get(&entry.name) {
            entry.tool_count = tools.len();
            entry.tool_names = tools.clone();
        }
        if let Some(failed) = report
            .failed_servers
            .iter()
            .find(|f| f.server_name == entry.name)
        {
            entry.error = Some(failed.error.to_string());
        }
        if let Some(uns) = report
            .unsupported_servers
            .iter()
            .find(|u| u.server_name == entry.name)
        {
            entry.unsupported_reason = Some(uns.reason.clone());
        }
    }

    let total = report.tools.len();
    McpRuntimeStatus {
        summary: format!(
            "{} 个 server / {} 个工具 / {} 失败 / {} 不支持",
            enabled.len(),
            total,
            report.failed_servers.len(),
            report.unsupported_servers.len()
        ),
        total_tools: total,
        servers: entries,
    }
}

/// Replace characters that some LLM providers reject in tool names.
/// Allowed: alphanumeric, underscore, hyphen.
fn sanitize_name_segment(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

impl DesktopMcp {
    /// Invoke an MCP tool by its **display** name (the one we sent to the
    /// model). Returns the tool's textual result.
    pub fn call(&self, display_name: &str, input: &JsonValue) -> Result<String, String> {
        let qualified = self
            .name_map
            .get(display_name)
            .ok_or_else(|| format!("unknown MCP tool: {display_name}"))?
            .clone();
        let mut manager = self
            .manager
            .lock()
            .map_err(|_| "MCP manager mutex poisoned".to_string())?;
        let response = self
            .rt
            .block_on(manager.call_tool(&qualified, Some(input.clone())))
            .map_err(|e| format!("MCP {display_name}: {e:?}"))?;

        // Extract result.content as concatenated text (MCP returns rich
        // content blocks — we keep it simple and stitch the text bits).
        // McpToolCallContent puts variant fields in `data` map. For text
        // content the spec is { "type": "text", "text": "..." }.
        let text = response
            .result
            .map(|r| {
                r.content
                    .into_iter()
                    .filter_map(|c| {
                        if c.kind == "text" {
                            c.data.get("text").and_then(|v| v.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_specs_returns_none() {
        let result = init(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn disabled_specs_return_none() {
        let specs = vec![McpServerSpec {
            name: "x".into(),
            command: "echo".into(),
            args: vec![],
            enabled: false,
        }];
        let result = init(&specs).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_command_returns_none() {
        let specs = vec![McpServerSpec {
            name: "x".into(),
            command: String::new(),
            args: vec![],
            enabled: true,
        }];
        let result = init(&specs).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn sanitize_drops_disallowed_chars() {
        assert_eq!(sanitize_name_segment("foo/bar"), "foo_bar");
        assert_eq!(sanitize_name_segment("a:b.c"), "a_b_c");
        assert_eq!(sanitize_name_segment("ok_name-123"), "ok_name-123");
    }
}
