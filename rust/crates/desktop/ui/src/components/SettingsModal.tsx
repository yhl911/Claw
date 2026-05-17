import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// `mcp_servers` and `hooks` get their own dedicated panels now
// (MCPPanel / HooksPanel). We keep them in the Config type as opaque
// pass-through arrays so save_settings round-trips them — without these
// fields the panels' values would be wiped whenever the user opens this
// modal and clicks save.
interface McpServerSpec {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
}

interface HookSpec {
  id: string;
  event: string;
  command: string;
  enabled: boolean;
  timeout_secs: number;
}

interface Config {
  model: string;
  api_key: string;
  base_url: string;
  opc_mode: boolean;
  thinking_mode: boolean;
  auto_dream: boolean;
  auto_dream_mode: string;
  mcp_servers: McpServerSpec[];
  hooks: HookSpec[];
  github_token: string;
  budget_daily_usd: number;
  budget_monthly_usd: number;
  permission_mode: string;
  auto_compact_threshold: number;
}

interface Props {
  onClose: () => void;
  onSaved: () => void;
}

export function SettingsModal({ onClose, onSaved }: Props) {
  const [config, setConfig] = useState<Config>({
    model: "claude-opus-4-6",
    api_key: "",
    base_url: "",
    opc_mode: true,
    thinking_mode: false,
    auto_dream: false,
    auto_dream_mode: "review",
    mcp_servers: [],
    hooks: [],
    github_token: "",
    budget_daily_usd: 0,
    budget_monthly_usd: 0,
    permission_mode: "danger-full-access",
    auto_compact_threshold: 0,
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);

  useEffect(() => {
    invoke<Config>("get_settings").then(setConfig).catch(() => {});
  }, []);

  async function handleSave() {
    setSaving(true);
    setError(null);
    setSuccess(false);
    try {
      await invoke("save_settings", { config });
      setSuccess(true);
      setTimeout(() => {
        onSaved(); // onSaved handles closing the modal
      }, 800);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[480px] max-h-[90vh] shadow-2xl border border-[#333] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333] flex-shrink-0">
          <h2 className="text-base font-semibold text-[#e5e5e5]">Settings</h2>
          <button
            onClick={onClose}
            className="text-[#666] hover:text-[#aaa] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        {/* Body — scrollable */}
        <div className="px-6 py-5 space-y-4 overflow-y-auto flex-1">
          {/* Model */}
          <div>
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              Model
            </label>
            <input
              type="text"
              value={config.model}
              onChange={(e) => setConfig({ ...config, model: e.target.value })}
              placeholder="e.g. claude-opus-4-6 or deepseek-v4-flash (auto-prefixed openai/)"
              className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2.5 border border-[#444] focus:outline-none focus:border-[#ff8c00] placeholder-[#555] transition-colors"
            />
          </div>

          {/* API Key */}
          <div>
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              API Key
            </label>
            <input
              type="password"
              value={config.api_key}
              onChange={(e) => setConfig({ ...config, api_key: e.target.value })}
              placeholder="sk-..."
              className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2.5 border border-[#444] focus:outline-none focus:border-[#ff8c00] placeholder-[#555] transition-colors font-mono"
            />
          </div>

          {/* Base URL */}
          <div>
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              Base URL{" "}
              <span className="text-[#555] font-normal">(without /chat/completions)</span>
            </label>
            <input
              type="text"
              value={config.base_url}
              onChange={(e) => setConfig({ ...config, base_url: e.target.value })}
              placeholder="https://tokenhub.tencentmaas.com/v1"
              className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2.5 border border-[#444] focus:outline-none focus:border-[#ff8c00] placeholder-[#555] transition-colors font-mono"
            />
          </div>

          {/* OPC Mode */}
          <div className="flex items-center justify-between py-1">
            <div>
              <p className="text-sm text-[#e5e5e5]">OPC CEO Mode</p>
              <p className="text-xs text-[#666] mt-0.5">
                Inject CEO agent system prompt for one-person company workflows
              </p>
            </div>
            <button
              onClick={() => setConfig({ ...config, opc_mode: !config.opc_mode })}
              className={`w-11 h-6 rounded-full transition-colors relative flex-shrink-0 ${
                config.opc_mode ? "bg-[#ff8c00]" : "bg-[#444]"
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 w-5 h-5 bg-white rounded-full shadow transition-transform ${
                  config.opc_mode ? "translate-x-5" : "translate-x-0"
                }`}
              />
            </button>
          </div>

          {/* Thinking Mode */}
          <div className="flex items-center justify-between py-1">
            <div>
              <p className="text-sm text-[#e5e5e5]">Thinking Mode (DeepSeek)</p>
              <p className="text-xs text-[#666] mt-0.5">
                仅对 deepseek-v* / deepseek-reasoner 生效。开启后多轮 + 工具调用更复杂，建议关闭。
              </p>
            </div>
            <button
              onClick={() => setConfig({ ...config, thinking_mode: !config.thinking_mode })}
              className={`w-11 h-6 rounded-full transition-colors relative flex-shrink-0 ${
                config.thinking_mode ? "bg-[#ff8c00]" : "bg-[#444]"
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 w-5 h-5 bg-white rounded-full shadow transition-transform ${
                  config.thinking_mode ? "translate-x-5" : "translate-x-0"
                }`}
              />
            </button>
          </div>

          {/* Auto Dreaming */}
          <div className="border-t border-[#333] pt-3">
            <div className="flex items-center justify-between py-1">
              <div>
                <p className="text-sm text-[#e5e5e5]">🌙 Auto Dreaming</p>
                <p className="text-xs text-[#666] mt-0.5">
                  清空会话时自动整合长期记忆
                </p>
              </div>
              <button
                onClick={() => setConfig({ ...config, auto_dream: !config.auto_dream })}
                className={`w-11 h-6 rounded-full transition-colors relative flex-shrink-0 ${
                  config.auto_dream ? "bg-[#ff8c00]" : "bg-[#444]"
                }`}
              >
                <span
                  className={`absolute top-0.5 left-0.5 w-5 h-5 bg-white rounded-full shadow transition-transform ${
                    config.auto_dream ? "translate-x-5" : "translate-x-0"
                  }`}
                />
              </button>
            </div>

            {config.auto_dream && (
              <div className="mt-2 ml-1 flex gap-2">
                <label className="flex items-center gap-1.5 text-xs text-[#aaa] cursor-pointer">
                  <input
                    type="radio"
                    name="dream_mode"
                    value="review"
                    checked={config.auto_dream_mode === "review"}
                    onChange={() => setConfig({ ...config, auto_dream_mode: "review" })}
                    className="accent-[#ff8c00]"
                  />
                  审核（推荐）
                </label>
                <label className="flex items-center gap-1.5 text-xs text-[#aaa] cursor-pointer">
                  <input
                    type="radio"
                    name="dream_mode"
                    value="apply"
                    checked={config.auto_dream_mode === "apply"}
                    onChange={() => setConfig({ ...config, auto_dream_mode: "apply" })}
                    className="accent-[#ff8c00]"
                  />
                  自动应用
                </label>
              </div>
            )}
          </div>

          {/* Auto-compact threshold */}
          <div className="border-t border-[#333] pt-3">
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              🗜️ 自动压缩阈值 <span className="text-[#555]">(0 = 关闭)</span>
            </label>
            <div className="flex items-center gap-2">
              <input
                type="range"
                min="0"
                max="0.9"
                step="0.05"
                value={config.auto_compact_threshold}
                onChange={(e) =>
                  setConfig({
                    ...config,
                    auto_compact_threshold: parseFloat(e.target.value),
                  })
                }
                className="flex-1 accent-[#ff8c00]"
              />
              <span className="text-xs text-[#ccc] font-mono w-12 text-right">
                {config.auto_compact_threshold > 0
                  ? `${Math.round(config.auto_compact_threshold * 100)}%`
                  : "off"}
              </span>
            </div>
            <p className="text-xs text-[#666] mt-1">
              当上下文使用率超过此值时，自动把旧消息浓缩成摘要。推荐 70% — 模型在
              20-40% 就开始"Lost in the Middle"。
            </p>
          </div>

          {/* Permission mode */}
          <div className="border-t border-[#333] pt-3">
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              工具权限模式
            </label>
            <select
              value={config.permission_mode}
              onChange={(e) =>
                setConfig({ ...config, permission_mode: e.target.value })
              }
              className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2 border border-[#444] focus:outline-none focus:border-[#ff8c00]"
            >
              <option value="danger-full-access">
                完全访问 (默认 — bash / 网络 / 写文件 全部放行)
              </option>
              <option value="workspace-write">
                工作区写入 (限制 bash 等高危操作)
              </option>
              <option value="read-only">
                只读 (仅读文件、搜索 — 演示用)
              </option>
            </select>
            <p className="text-xs text-[#666] mt-1">
              非默认会让 CEO 报"环境权限受限"。本地桌面默认就该是完全访问。
            </p>
          </div>

          {/* Budget caps */}
          <div className="border-t border-[#333] pt-3">
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              预算上限 <span className="text-[#555]">(USD，0 = 无限制)</span>
            </label>
            <div className="flex gap-2">
              <div className="flex-1">
                <input
                  type="number"
                  min="0"
                  step="0.5"
                  value={config.budget_daily_usd || ""}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      budget_daily_usd: parseFloat(e.target.value) || 0,
                    })
                  }
                  placeholder="日上限"
                  className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2 border border-[#444] focus:outline-none focus:border-[#ff8c00] placeholder-[#555]"
                />
                <p className="text-[10px] text-[#666] mt-0.5">每日上限</p>
              </div>
              <div className="flex-1">
                <input
                  type="number"
                  min="0"
                  step="1"
                  value={config.budget_monthly_usd || ""}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      budget_monthly_usd: parseFloat(e.target.value) || 0,
                    })
                  }
                  placeholder="月上限"
                  className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2 border border-[#444] focus:outline-none focus:border-[#ff8c00] placeholder-[#555]"
                />
                <p className="text-[10px] text-[#666] mt-0.5">每月上限</p>
              </div>
            </div>
            <p className="text-xs text-[#666] mt-1">
              超过上限时新对话和长跑任务会被阻止启动，避免意外巨额账单。
            </p>
          </div>

          {/* GitHub token (for skills import) */}
          <div className="border-t border-[#333] pt-3">
            <label className="block text-xs font-medium text-[#888] mb-1.5">
              GitHub Token <span className="text-[#555]">(可选)</span>
            </label>
            <input
              type="password"
              value={config.github_token}
              onChange={(e) => setConfig({ ...config, github_token: e.target.value })}
              placeholder="ghp_... — 用于 Skills 从 GitHub 拉取（提升限额、绕过限流）"
              className="w-full bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2.5 border border-[#444] focus:outline-none focus:border-[#ff8c00] placeholder-[#555] transition-colors font-mono"
            />
            <p className="text-xs text-[#666] mt-1">
              个人 PAT 即可，无需任何 scope。留空则未认证（公开仓库可读，但有 60/小时 限额）。
            </p>
          </div>


          {error && (
            <div className="px-3 py-2 bg-red-900/30 border border-red-800 rounded-lg text-xs text-red-300">
              {error}
            </div>
          )}
          {success && (
            <div className="px-3 py-2 bg-green-900/30 border border-green-800 rounded-lg text-xs text-green-300">
              Settings saved. Reconnecting…
            </div>
          )}
        </div>

        {/* Footer — always visible at bottom */}
        <div className="flex justify-end gap-3 px-6 py-4 border-t border-[#333] flex-shrink-0">
          <button
            onClick={onClose}
            className="px-4 py-2 text-sm text-[#888] hover:text-[#ccc] transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            disabled={saving || !config.model.trim()}
            className="px-5 py-2 text-sm bg-[#ff8c00] text-white rounded-lg hover:bg-[#e07800] disabled:opacity-50 transition-colors font-medium"
          >
            {saving ? "Saving…" : "Save & Reconnect"}
          </button>
        </div>
      </div>
    </div>
  );
}
