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

/**
 * Display of pinned decision anchors for the current session.
 *
 * Anchors are facts the model (or user-via-model) decided should remain
 * authoritative throughout the session — names, technical choices,
 * style guidelines. They're re-injected into the system prompt on every
 * turn, so they survive context drift and compaction.
 */
export function AnchorsPanel({ onClose }: Props) {
  const [anchors, setAnchors] = useState<AnchorEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    try {
      const list = await invoke<AnchorEntry[]>("list_anchors");
      setAnchors(list);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function handleRemove(title: string) {
    try {
      await invoke("remove_anchor", { title });
      refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-[#1a1a1a] border border-[#333] rounded-xl w-full max-w-2xl max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-[#333]">
          <div className="flex items-center gap-2">
            <span className="text-lg">📌</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">
              Pinned Decisions
            </h2>
            <span className="text-xs text-[#666]">
              — 本会话锚点，每轮都会注入系统提示
            </span>
          </div>
          <div className="flex items-center gap-3">
            <button
              onClick={refresh}
              className="text-xs text-[#888] hover:text-[#e5e5e5] transition-colors"
            >
              ⟳
            </button>
            <button
              onClick={onClose}
              className="text-[#888] hover:text-[#e5e5e5] transition-colors text-xl leading-none"
            >
              ×
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-3">
          <p className="text-xs text-[#666] leading-relaxed">
            模型可以通过 <code className="text-[#aaa]">pin_decision</code> 工具把关键决策固定到系统提示，避免长会话忘记早期决定。压缩历史时锚点不会丢。
          </p>

          {error && (
            <div className="text-xs text-red-400 bg-red-900/20 p-2 rounded">
              {error}
            </div>
          )}

          {anchors.length === 0 ? (
            <div className="text-center py-10 text-xs text-[#555]">
              (本会话还没有锚点)
            </div>
          ) : (
            <ul className="space-y-2">
              {anchors.map((a) => (
                <li
                  key={`${a.pinned_at_secs}-${a.title}`}
                  className="bg-[#222] rounded-lg p-3 border border-[#333] group"
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="text-sm text-[#e5e5e5] font-medium flex-1">
                      {a.title}
                    </div>
                    <button
                      onClick={() => handleRemove(a.title)}
                      className="text-[10px] text-[#555] hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity"
                      title="删除此锚点"
                    >
                      ✕
                    </button>
                  </div>
                  <div className="text-xs text-[#aaa] mt-1 leading-relaxed">
                    {a.rationale}
                  </div>
                  <div className="text-[10px] text-[#555] mt-1.5">
                    {formatRelative(a.pinned_at_secs)}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

function formatRelative(secs: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - secs;
  if (diff < 60) return `${diff}s 前`;
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}
