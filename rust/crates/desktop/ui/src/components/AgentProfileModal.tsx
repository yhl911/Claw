import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  role: string;
  onClose: () => void;
}

interface Proposal {
  role: string;
  content: string;
  rationale: string;
  previous: string;
}

const ROLE_LABELS: Record<string, string> = {
  "opc-product": "产品",
  "opc-engineering": "工程",
  "opc-finance": "财务",
  "opc-marketing": "市场",
  "opc-sales": "销售",
  "opc-ops": "运营",
  "opc-legal": "法务",
};

export function AgentProfileModal({ role, onClose }: Props) {
  const [phase, setPhase] = useState<"running" | "review" | "done">("running");
  const [proposal, setProposal] = useState<Proposal | null>(null);
  const [edited, setEdited] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<Proposal>("run_agent_profile_dream", { role })
      .then((p) => {
        setProposal(p);
        setEdited(p.content);
        setPhase("review");
      })
      .catch((e) => {
        setError(String(e));
        setPhase("review");
      });
  }, [role]);

  async function handleApply() {
    if (!proposal) return;
    setError(null);
    try {
      await invoke("apply_agent_profile", {
        proposal: { ...proposal, content: edited },
      });
      setPhase("done");
      setTimeout(onClose, 1200);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-[60]"
      onClick={(e) => e.target === e.currentTarget && phase !== "running" && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[820px] max-w-[95vw] h-[80vh] flex flex-col shadow-2xl border border-[#333]"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333]">
          <div>
            <h2 className="text-base font-semibold text-[#e5e5e5]">
              📋 {ROLE_LABELS[role] ?? role} 角色画像
            </h2>
            {proposal?.rationale && (
              <p className="text-xs text-[#888] mt-1">{proposal.rationale}</p>
            )}
          </div>
          <button
            onClick={onClose}
            disabled={phase === "running"}
            className="text-[#666] hover:text-[#aaa] text-xl leading-none disabled:opacity-30"
          >
            ×
          </button>
        </div>

        {phase === "running" && (
          <div className="flex-1 flex flex-col items-center justify-center text-center px-8">
            <div className="text-4xl mb-4 animate-pulse">📋</div>
            <p className="text-sm text-[#aaa]">分析 sub-agent 历史，生成画像…</p>
          </div>
        )}

        {error && (
          <div className="mx-6 mt-4 px-3 py-2 bg-red-900/30 border border-red-800 rounded-lg text-xs text-red-300 whitespace-pre-wrap">
            {error}
          </div>
        )}

        {phase === "review" && proposal && (
          <div className="flex-1 grid grid-cols-2 min-h-0">
            <div className="border-r border-[#333] flex flex-col min-h-0">
              <p className="px-3 py-1.5 text-[10px] text-[#666] uppercase border-b border-[#333] bg-[#1a1a1a]">
                当前画像
              </p>
              <pre className="flex-1 overflow-auto p-3 text-xs text-[#888] whitespace-pre-wrap font-mono">
                {proposal.previous || "(还没有画像)"}
              </pre>
            </div>
            <div className="flex flex-col min-h-0">
              <p className="px-3 py-1.5 text-[10px] text-[#ff8c00] uppercase border-b border-[#333] bg-[#1a1a1a]">
                提议（可编辑）
              </p>
              <textarea
                value={edited}
                onChange={(e) => setEdited(e.target.value)}
                className="flex-1 bg-[#1a1a1a] text-[#e5e5e5] text-xs p-3 font-mono resize-none focus:outline-none focus:bg-[#1e1e1e]"
              />
            </div>
          </div>
        )}

        {phase === "done" && (
          <div className="flex-1 flex flex-col items-center justify-center px-8">
            <div className="text-4xl mb-4">✓</div>
            <p className="text-sm text-[#aaa]">画像已保存</p>
          </div>
        )}

        {phase === "review" && (
          <div className="flex justify-end gap-3 px-6 py-3 border-t border-[#333]">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm text-[#888] hover:text-[#ccc]"
            >
              取消
            </button>
            <button
              onClick={handleApply}
              disabled={!proposal || !edited.trim()}
              className="px-5 py-2 text-sm bg-[#ff8c00] text-white rounded-lg hover:bg-[#e07800] disabled:opacity-50 font-medium"
            >
              应用画像
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
