import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  agentId: string;
  onClose: () => void;
}

interface AgentDetail {
  manifest: Record<string, unknown>;
  output: string | null;
}

export function AgentDetailModal({ agentId, onClose }: Props) {
  const [detail, setDetail] = useState<AgentDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dismissing, setDismissing] = useState(false);

  useEffect(() => {
    let live = true;
    const refresh = () =>
      invoke<AgentDetail>("read_agent_detail", { agentId })
        .then((d) => {
          if (live) setDetail(d);
        })
        .catch((e) => {
          if (live) setError(String(e));
        });
    refresh();
    const timer = setInterval(refresh, 2000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [agentId]);

  async function handleDismiss() {
    setDismissing(true);
    try {
      await invoke("dismiss_agent", { agentId });
      onClose();
    } catch (e) {
      setError(String(e));
      setDismissing(false);
    }
  }

  const status =
    (detail?.manifest?.status as string | undefined) ?? "unknown";
  const subagentType =
    (detail?.manifest?.subagentType as string | undefined) ?? "";
  const description =
    (detail?.manifest?.description as string | undefined) ?? "";
  const errMessage =
    (detail?.manifest?.error as string | undefined) ?? null;
  const laneEvents =
    (detail?.manifest?.laneEvents as Array<Record<string, unknown>>) ?? [];

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[640px] max-h-[80vh] flex flex-col shadow-2xl border border-[#333]"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333]">
          <div className="min-w-0">
            <h2 className="text-base font-semibold text-[#e5e5e5] truncate">
              {subagentType || agentId}
            </h2>
            <p className="text-xs text-[#666] mt-0.5 truncate">{agentId}</p>
          </div>
          <div className="flex items-center gap-3 flex-shrink-0 ml-4">
            <span
              className={`px-2 py-0.5 text-xs rounded ${
                status === "running"
                  ? "bg-yellow-900/40 text-yellow-300"
                  : status === "completed"
                    ? "bg-green-900/40 text-green-300"
                    : status === "failed"
                      ? "bg-red-900/40 text-red-300"
                      : "bg-gray-800 text-gray-400"
              }`}
            >
              {status}
            </span>
            <button
              onClick={onClose}
              className="text-[#666] hover:text-[#aaa] transition-colors text-xl leading-none"
            >
              ×
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-6 py-4 space-y-4">
          {error && (
            <div className="px-3 py-2 bg-red-900/30 border border-red-800 rounded-lg text-xs text-red-300">
              {error}
            </div>
          )}

          {description && (
            <section>
              <h3 className="text-xs font-medium text-[#888] mb-1.5 uppercase tracking-wider">
                委派任务
              </h3>
              <p className="text-sm text-[#e5e5e5] whitespace-pre-wrap">
                {description}
              </p>
            </section>
          )}

          {errMessage && (
            <section>
              <h3 className="text-xs font-medium text-red-400 mb-1.5 uppercase tracking-wider">
                错误
              </h3>
              <pre className="text-xs text-red-300 bg-red-900/20 rounded-lg p-3 overflow-x-auto whitespace-pre-wrap">
                {errMessage}
              </pre>
            </section>
          )}

          {laneEvents.length > 0 && (
            <section>
              <h3 className="text-xs font-medium text-[#888] mb-1.5 uppercase tracking-wider">
                进度事件 ({laneEvents.length})
              </h3>
              <div className="space-y-1 max-h-32 overflow-y-auto">
                {laneEvents.map((evt, i) => (
                  <div
                    key={i}
                    className="text-xs text-[#aaa] bg-[#1a1a1a] rounded px-2 py-1 font-mono"
                  >
                    {JSON.stringify(evt)}
                  </div>
                ))}
              </div>
            </section>
          )}

          {detail?.output && (
            <section>
              <h3 className="text-xs font-medium text-[#888] mb-1.5 uppercase tracking-wider">
                输出
              </h3>
              <pre className="text-xs text-[#ddd] bg-[#1a1a1a] rounded-lg p-3 overflow-x-auto whitespace-pre-wrap leading-relaxed">
                {detail.output}
              </pre>
            </section>
          )}

          {!detail && !error && (
            <p className="text-sm text-[#666] text-center py-8">加载中…</p>
          )}
        </div>

        <div className="flex justify-between items-center px-6 py-3 border-t border-[#333]">
          <p className="text-xs text-[#555]">每 2 秒自动刷新</p>
          <button
            onClick={handleDismiss}
            disabled={dismissing || status === "running"}
            title={
              status === "running"
                ? "运行中的 agent 无法直接 dismiss（线程不可中断）"
                : "从列表中移除"
            }
            className="px-3 py-1.5 text-xs text-[#aaa] border border-[#444] rounded-lg hover:bg-[#333] hover:text-white disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
          >
            {dismissing ? "处理中…" : "Dismiss"}
          </button>
        </div>
      </div>
    </div>
  );
}
