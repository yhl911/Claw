import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface HookSpec {
  id: string;
  event: string;
  command: string;
  enabled: boolean;
  timeout_secs: number;
}

const HOOK_EVENTS = [
  "after_turn",
  "before_clear_session",
  "after_clear_session",
  "after_long_task",
  "after_agent",
];

const EVENT_HINTS: Record<string, string> = {
  after_turn: "每次 send_message 完成后",
  before_clear_session: "清空会话之前（可用于备份）",
  after_clear_session: "清空会话之后",
  after_long_task: "长跑任务结束（done / failed / cancelled）",
  after_agent: "OPC sub-agent 完成时",
};

interface Props {
  onClose: () => void;
  onSaved: () => void;
}

export function HooksPanel({ onClose, onSaved }: Props) {
  const [hooks, setHooks] = useState<HookSpec[]>([]);
  const [fullConfig, setFullConfig] = useState<Record<string, unknown> | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);

  useEffect(() => {
    invoke<Record<string, unknown>>("get_settings")
      .then((cfg) => {
        setFullConfig(cfg);
        setHooks((cfg.hooks as HookSpec[]) ?? []);
      })
      .catch((e) => setError(String(e)));
  }, []);

  async function save() {
    if (!fullConfig) return;
    setSaving(true);
    setError(null);
    setSuccess(false);
    try {
      await invoke("save_settings", { config: { ...fullConfig, hooks } });
      setSuccess(true);
      setTimeout(() => onSaved(), 600);
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
            <span className="text-lg">🔔</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">Hooks</h2>
          </div>
          <button
            onClick={onClose}
            className="text-[#888] hover:text-[#e5e5e5] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-3">
          <div className="text-xs text-[#666] space-y-1">
            <p>事件发生时运行 shell 命令。JSON 上下文通过 stdin 传入命令。</p>
            <p>
              典型用例：长跑任务完成时发桌面通知（
              <code className="text-[#aaa]">osascript -e &apos;display notification ...&apos;</code>
              ）；清会话前自动 git commit；推送到 webhook。
            </p>
          </div>

          <div className="flex justify-end">
            <button
              onClick={() =>
                setHooks((h) => [
                  ...h,
                  {
                    id: `hook-${Date.now()}`,
                    event: "after_long_task",
                    command: "",
                    enabled: true,
                    timeout_secs: 0,
                  },
                ])
              }
              className="text-xs px-3 py-1.5 bg-[#2d2d2d] hover:bg-[#3a3a3a] text-[#aaa] hover:text-[#ff8c00] rounded border border-[#444] transition-colors"
            >
              + 添加 hook
            </button>
          </div>

          {hooks.length === 0 ? (
            <p className="text-xs text-[#555] py-4 text-center">
              还没有 hook。点击"+ 添加 hook"开始。
            </p>
          ) : (
            <div className="space-y-2">
              {hooks.map((h, idx) => (
                <div
                  key={h.id}
                  className="p-2 bg-[#222] border border-[#333] rounded space-y-1.5"
                >
                  <div className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={h.enabled}
                      onChange={(e) =>
                        setHooks((hs) =>
                          hs.map((x, i) =>
                            i === idx ? { ...x, enabled: e.target.checked } : x,
                          ),
                        )
                      }
                      className="accent-[#ff8c00]"
                    />
                    <select
                      value={h.event}
                      onChange={(e) =>
                        setHooks((hs) =>
                          hs.map((x, i) =>
                            i === idx ? { ...x, event: e.target.value } : x,
                          ),
                        )
                      }
                      className="bg-[#0d0d0d] text-[#e5e5e5] text-xs rounded px-2 py-1 border border-[#444] focus:outline-none focus:border-[#ff8c00]"
                    >
                      {HOOK_EVENTS.map((ev) => (
                        <option key={ev} value={ev}>
                          {ev}
                        </option>
                      ))}
                    </select>
                    <span className="text-xs text-[#666] flex-1 truncate">
                      {EVENT_HINTS[h.event] ?? ""}
                    </span>
                    <button
                      onClick={() => setHooks((hs) => hs.filter((_, i) => i !== idx))}
                      title="移除"
                      className="text-[#666] hover:text-red-400 text-xs px-1"
                    >
                      ✕
                    </button>
                  </div>
                  <input
                    type="text"
                    value={h.command}
                    onChange={(e) =>
                      setHooks((hs) =>
                        hs.map((x, i) =>
                          i === idx ? { ...x, command: e.target.value } : x,
                        ),
                      )
                    }
                    placeholder='shell 命令，如：osascript -e &apos;display notification "task done"&apos;'
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
