import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface McpServerSpec {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
}

interface McpServerStatus {
  name: string;
  tool_count: number;
  tool_names: string[];
  error: string | null;
  unsupported_reason: string | null;
}

interface McpRuntimeStatus {
  summary: string;
  total_tools: number;
  servers: McpServerStatus[];
}

interface Props {
  onClose: () => void;
  onSaved: () => void;
}

export function MCPPanel({ onClose, onSaved }: Props) {
  const [servers, setServers] = useState<McpServerSpec[]>([]);
  const [fullConfig, setFullConfig] = useState<Record<string, unknown> | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);
  const [status, setStatus] = useState<McpRuntimeStatus | null>(null);
  const [probing, setProbing] = useState(false);

  useEffect(() => {
    invoke<Record<string, unknown>>("get_settings")
      .then((cfg) => {
        setFullConfig(cfg);
        setServers((cfg.mcp_servers as McpServerSpec[]) ?? []);
      })
      .catch((e) => setError(String(e)));
  }, []);

  async function probe() {
    setProbing(true);
    setError(null);
    try {
      // Probe uses the persisted config — save first so the user sees the
      // effect of their unsaved edits.
      if (fullConfig) {
        await invoke("save_settings", { config: { ...fullConfig, mcp_servers: servers } });
      }
      const s = await invoke<McpRuntimeStatus>("get_mcp_status");
      setStatus(s);
    } catch (e) {
      setError(String(e));
    } finally {
      setProbing(false);
    }
  }

  async function save() {
    if (!fullConfig) return;
    setSaving(true);
    setError(null);
    setSuccess(false);
    try {
      await invoke("save_settings", { config: { ...fullConfig, mcp_servers: servers } });
      setSuccess(true);
      setTimeout(() => {
        onSaved();
      }, 600);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-[#1a1a1a] border border-[#333] rounded-xl w-full max-w-3xl max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-[#333]">
          <div className="flex items-center gap-2">
            <span className="text-lg">🧩</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">MCP Servers</h2>
          </div>
          <button
            onClick={onClose}
            className="text-[#888] hover:text-[#e5e5e5] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-3">
          <p className="text-xs text-[#666]">
            外部工具进程（stdio 协议）。保存后会在下一次会话生效，长跑任务也会自动接入。
          </p>

          <div className="flex gap-2">
            <button
              onClick={probe}
              disabled={probing || servers.length === 0}
              className="text-xs px-3 py-1.5 bg-[#2d2d2d] hover:bg-[#3a3a3a] disabled:opacity-40 text-[#aaa] hover:text-[#ff8c00] rounded border border-[#444] transition-colors"
              title="启动配置中的 MCP server 探测可用工具"
            >
              {probing ? "测试中..." : "测试连接"}
            </button>
            <button
              onClick={() =>
                setServers((s) => [
                  ...s,
                  { name: "", command: "", args: [], enabled: true },
                ])
              }
              className="text-xs px-3 py-1.5 bg-[#2d2d2d] hover:bg-[#3a3a3a] text-[#aaa] hover:text-[#ff8c00] rounded border border-[#444] transition-colors"
            >
              + 添加 server
            </button>
          </div>

          {status && (
            <div className="px-3 py-2 bg-[#0d0d0d] border border-[#2a2a2a] rounded text-xs space-y-1">
              <div className="text-[#aaa]">{status.summary}</div>
              {status.servers.map((s) => (
                <div key={s.name} className="flex flex-wrap items-center gap-1">
                  <span
                    className={`w-1.5 h-1.5 rounded-full ${
                      s.error
                        ? "bg-red-500"
                        : s.unsupported_reason
                          ? "bg-yellow-500"
                          : s.tool_count > 0
                            ? "bg-green-500"
                            : "bg-[#555]"
                    }`}
                  />
                  <span className="text-[#e5e5e5]">{s.name}</span>
                  <span className="text-[#666]">
                    {s.error
                      ? `错误：${s.error}`
                      : s.unsupported_reason
                        ? `不支持：${s.unsupported_reason}`
                        : `${s.tool_count} 个工具`}
                  </span>
                  {s.tool_names.length > 0 && (
                    <span className="text-[#555] truncate" title={s.tool_names.join(", ")}>
                      ({s.tool_names.slice(0, 4).join(", ")}
                      {s.tool_names.length > 4 ? "…" : ""})
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}

          {servers.length === 0 ? (
            <p className="text-xs text-[#555] py-4 text-center">
              还没有配置 MCP server。点击"+ 添加 server"开始。
            </p>
          ) : (
            <div className="space-y-2">
              {servers.map((srv, idx) => (
                <div
                  key={idx}
                  className="p-2 bg-[#222] border border-[#333] rounded space-y-1.5"
                >
                  <div className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={srv.enabled}
                      onChange={(e) =>
                        setServers((s) =>
                          s.map((x, i) =>
                            i === idx ? { ...x, enabled: e.target.checked } : x,
                          ),
                        )
                      }
                      title="启用此 server"
                      className="accent-[#ff8c00]"
                    />
                    <input
                      type="text"
                      value={srv.name}
                      onChange={(e) =>
                        setServers((s) =>
                          s.map((x, i) =>
                            i === idx ? { ...x, name: e.target.value } : x,
                          ),
                        )
                      }
                      placeholder="名称 (e.g. github)"
                      className="flex-1 bg-[#0d0d0d] text-[#e5e5e5] text-xs rounded px-2 py-1 border border-[#444] focus:outline-none focus:border-[#ff8c00]"
                    />
                    <button
                      onClick={() => setServers((s) => s.filter((_, i) => i !== idx))}
                      title="移除"
                      className="text-[#666] hover:text-red-400 text-xs px-1"
                    >
                      ✕
                    </button>
                  </div>
                  <input
                    type="text"
                    value={srv.command}
                    onChange={(e) =>
                      setServers((s) =>
                        s.map((x, i) =>
                          i === idx ? { ...x, command: e.target.value } : x,
                        ),
                      )
                    }
                    placeholder="命令 (e.g. npx)"
                    className="w-full bg-[#0d0d0d] text-[#e5e5e5] text-xs rounded px-2 py-1 border border-[#444] focus:outline-none focus:border-[#ff8c00] font-mono"
                  />
                  <input
                    type="text"
                    value={srv.args.join(" ")}
                    onChange={(e) =>
                      setServers((s) =>
                        s.map((x, i) =>
                          i === idx
                            ? {
                                ...x,
                                args: e.target.value
                                  .split(/\s+/)
                                  .filter((a) => a.length > 0),
                              }
                            : x,
                        ),
                      )
                    }
                    placeholder="参数（空格分隔，e.g. -y @modelcontextprotocol/server-github）"
                    className="w-full bg-[#0d0d0d] text-[#e5e5e5] text-xs rounded px-2 py-1 border border-[#444] focus:outline-none focus:border-[#ff8c00] font-mono"
                  />
                </div>
              ))}
            </div>
          )}

          {error && (
            <div className="px-3 py-2 bg-red-900/30 border border-red-800 rounded text-xs text-red-300">
              {error}
            </div>
          )}
          {success && (
            <div className="px-3 py-2 bg-green-900/30 border border-green-800 rounded text-xs text-green-300">
              已保存
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 px-5 py-3 border-t border-[#333]">
          <button
            onClick={onClose}
            className="text-xs px-3 py-1.5 text-[#aaa] hover:text-[#e5e5e5] transition-colors"
          >
            取消
          </button>
          <button
            onClick={save}
            disabled={saving || !fullConfig}
            className="text-xs px-3 py-1.5 bg-[#ff8c00] hover:bg-[#ff7700] disabled:opacity-50 text-black rounded transition-colors"
          >
            {saving ? "保存中..." : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
