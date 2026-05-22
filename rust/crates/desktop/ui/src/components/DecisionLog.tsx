import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface AnchorEntry {
  title: string;
  rationale: string;
  pinned_at_secs: number;
}

interface Props {
  onClose: () => void;
}

function relativeTime(secs: number): string {
  const diff = Math.floor(Date.now() / 1000) - secs;
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 7) return `${Math.floor(diff / 86400)} 天前`;
  const d = new Date(secs * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function DecisionLog({ onClose }: Props) {
  const [entries, setEntries] = useState<AnchorEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [removing, setRemoving] = useState<string | null>(null);

  async function load() {
    try {
      const list = await invoke<AnchorEntry[]>("list_anchors");
      setEntries(list);
    } catch {
      setEntries([]);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { load(); }, []);

  async function handleRemove(title: string) {
    setRemoving(title);
    try {
      await invoke("remove_anchor", { title });
      setEntries((prev) => prev.filter((e) => e.title !== title));
    } finally {
      setRemoving(null);
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[580px] max-h-[70vh] shadow-2xl border border-[#333] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333] flex-shrink-0">
          <div className="flex items-center gap-2">
            <span className="text-lg">📌</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">决策日志</h2>
            {entries.length > 0 && (
              <span className="text-xs text-[#666] bg-[#333] px-2 py-0.5 rounded-full">
                {entries.length}
              </span>
            )}
          </div>
          <button
            onClick={onClose}
            className="text-[#666] hover:text-[#aaa] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        {/* Sub-header */}
        <div className="px-6 py-2.5 border-b border-[#2a2a2a] flex-shrink-0">
          <p className="text-xs text-[#555]">
            CEO 用 <code className="bg-[#1a1a1a] px-1 py-0.5 rounded text-[#ff8c00]">pin_decision</code> 固定的重要判断，注入每次对话的系统提示，防止长会话遗忘。
          </p>
        </div>

        {/* List */}
        <div className="flex-1 overflow-y-auto px-4 py-3 min-h-0">
          {loading ? (
            <div className="flex items-center justify-center h-32 text-[#555] text-sm">
              加载中…
            </div>
          ) : entries.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-40 text-center px-6">
              <span className="text-3xl mb-3 opacity-40">📌</span>
              <p className="text-sm text-[#555]">还没有固定决策</p>
              <p className="text-xs text-[#444] mt-1">
                CEO 使用 pin_decision 工具后，重要判断会出现在这里
              </p>
            </div>
          ) : (
            <div className="space-y-2">
              {entries.map((entry) => (
                <div
                  key={entry.title}
                  className="group bg-[#1e1e1e] border border-[#2e2e2e] hover:border-[#3a3a3a] rounded-xl px-4 py-3 transition-colors"
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 mb-1">
                        <span className="text-xs text-[#ff8c00] font-mono">●</span>
                        <span className="text-sm font-medium text-[#e5e5e5] truncate">
                          {entry.title}
                        </span>
                      </div>
                      <p className="text-xs text-[#888] leading-relaxed pl-4">
                        {entry.rationale}
                      </p>
                    </div>
                    <div className="flex items-start gap-2 flex-shrink-0 pt-0.5">
                      <span className="text-xs text-[#444] whitespace-nowrap">
                        {relativeTime(entry.pinned_at_secs)}
                      </span>
                      <button
                        onClick={() => handleRemove(entry.title)}
                        disabled={removing === entry.title}
                        className="text-[#444] hover:text-red-400 transition-colors opacity-0 group-hover:opacity-100 text-base leading-none disabled:opacity-30"
                        title="移除此决策"
                      >
                        {removing === entry.title ? "…" : "×"}
                      </button>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Footer */}
        {entries.length > 0 && (
          <div className="px-6 py-3 border-t border-[#2a2a2a] flex-shrink-0">
            <p className="text-xs text-[#444]">
              共 {entries.length} 条决策 · 这些内容会注入每次对话的系统提示
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
