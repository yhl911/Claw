import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface DreamProposal {
  files: Record<string, string>;
  rationale: string;
}

interface DreamResult {
  proposal: DreamProposal;
  previous: Record<string, string>;
}

interface Props {
  onClose: () => void;
  /// If provided, skip the run_dream call and go straight to review with this
  /// pre-computed proposal (used for auto-dream pending events).
  initialResult?: DreamResult;
}

const FILE_LABELS: Record<string, string> = {
  "facts.md": "事实 Facts",
  "decisions.md": "决策 Decisions",
  "patterns.md": "模式 Patterns",
  "failures.md": "失败 Failures",
};

function fileLabel(name: string): string {
  if (FILE_LABELS[name]) return FILE_LABELS[name];
  if (name.startsWith("agent_profiles/")) {
    return `角色画像: ${name.replace("agent_profiles/", "").replace(".md", "")}`;
  }
  return name;
}

export function DreamReviewModal({ onClose, initialResult }: Props) {
  const [phase, setPhase] = useState<"running" | "review" | "applying" | "done">(
    initialResult ? "review" : "running",
  );
  const [result, setResult] = useState<DreamResult | null>(initialResult ?? null);
  const [editedFiles, setEditedFiles] = useState<Record<string, string>>(
    initialResult ? { ...initialResult.proposal.files } : {},
  );
  const [acceptedFiles, setAcceptedFiles] = useState<Set<string>>(
    initialResult ? new Set(Object.keys(initialResult.proposal.files)) : new Set(),
  );
  const [error, setError] = useState<string | null>(null);
  const [activeFile, setActiveFile] = useState<string | null>(
    initialResult ? Object.keys(initialResult.proposal.files)[0] ?? null : null,
  );

  useEffect(() => {
    if (initialResult) return; // already have proposal from auto-dream
    invoke<DreamResult>("run_dream")
      .then((r) => {
        setResult(r);
        setEditedFiles({ ...r.proposal.files });
        setAcceptedFiles(new Set(Object.keys(r.proposal.files)));
        setActiveFile(Object.keys(r.proposal.files)[0] ?? null);
        setPhase("review");
      })
      .catch((e) => {
        setError(String(e));
        setPhase("done");
      });
  }, [initialResult]);

  async function handleApply() {
    if (!result) return;
    setPhase("applying");
    setError(null);
    const filtered: Record<string, string> = {};
    for (const name of acceptedFiles) {
      filtered[name] = editedFiles[name] ?? "";
    }
    try {
      const changed = await invoke<string[]>("apply_dream", {
        proposal: { files: filtered, rationale: result.proposal.rationale },
      });
      console.log("[dream] applied changes:", changed);
      setPhase("done");
      setTimeout(onClose, 1200);
    } catch (e) {
      setError(String(e));
      setPhase("review");
    }
  }

  function toggleAccept(name: string) {
    setAcceptedFiles((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={(e) => e.target === e.currentTarget && phase !== "running" && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[900px] max-w-[95vw] h-[85vh] flex flex-col shadow-2xl border border-[#333]"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333]">
          <div>
            <h2 className="text-base font-semibold text-[#e5e5e5] flex items-center gap-2">
              <span>🌙</span> Dreaming — 长期记忆固化
            </h2>
            {result?.proposal.rationale && (
              <p className="text-xs text-[#888] mt-1">{result.proposal.rationale}</p>
            )}
          </div>
          <button
            onClick={onClose}
            disabled={phase === "running" || phase === "applying"}
            className="text-[#666] hover:text-[#aaa] transition-colors text-xl leading-none disabled:opacity-30"
          >
            ×
          </button>
        </div>

        {/* Body */}
        {phase === "running" && (
          <div className="flex-1 flex flex-col items-center justify-center text-center px-8">
            <div className="text-4xl mb-4 animate-pulse">🌙</div>
            <p className="text-sm text-[#aaa]">
              正在审视最近会话，提炼长期记忆…
            </p>
            <p className="text-xs text-[#555] mt-2">
              这可能需要 30 秒到 3 分钟，取决于会话长度和模型响应速度
            </p>
          </div>
        )}

        {error && phase !== "running" && (
          <div className="mx-6 mt-4 px-3 py-2 bg-red-900/30 border border-red-800 rounded-lg text-xs text-red-300 whitespace-pre-wrap">
            {error}
          </div>
        )}

        {phase === "review" && result && (
          <div className="flex-1 flex min-h-0">
            {/* File list */}
            <div className="w-56 border-r border-[#333] overflow-y-auto p-2 flex-shrink-0">
              <p className="text-[10px] text-[#666] uppercase px-2 mb-1.5">建议改动</p>
              {Object.keys(result.proposal.files).map((name) => {
                const accepted = acceptedFiles.has(name);
                const isActive = activeFile === name;
                const hadPrevious = (result.previous[name] ?? "").trim().length > 0;
                const isEmpty = !(editedFiles[name] ?? "").trim();
                return (
                  <div
                    key={name}
                    onClick={() => setActiveFile(name)}
                    className={`mb-1 px-2 py-1.5 rounded cursor-pointer flex items-start gap-2 transition-colors ${
                      isActive
                        ? "bg-[#2d2d2d] border border-[#ff8c00]/40"
                        : "hover:bg-[#2a2a2a]"
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={accepted}
                      onChange={(e) => {
                        e.stopPropagation();
                        toggleAccept(name);
                      }}
                      className="mt-0.5 accent-[#ff8c00]"
                      onClick={(e) => e.stopPropagation()}
                    />
                    <div className="min-w-0 flex-1">
                      <p className="text-xs text-[#e5e5e5] truncate">
                        {fileLabel(name)}
                      </p>
                      <p className="text-[10px] text-[#666] mt-0.5">
                        {isEmpty
                          ? "🗑 删除"
                          : hadPrevious
                            ? "✎ 修改"
                            : "+ 新建"}
                      </p>
                    </div>
                  </div>
                );
              })}
            </div>

            {/* Diff/Editor pane */}
            <div className="flex-1 flex flex-col min-w-0">
              {activeFile && (
                <>
                  <div className="px-4 py-2 border-b border-[#333] bg-[#1e1e1e] text-xs text-[#888] flex items-center justify-between">
                    <span className="font-mono">{activeFile}</span>
                    <span className="text-[10px] text-[#555]">可编辑</span>
                  </div>
                  <div className="flex-1 grid grid-cols-2 min-h-0">
                    {/* Previous */}
                    <div className="border-r border-[#333] flex flex-col min-h-0">
                      <p className="px-3 py-1.5 text-[10px] text-[#666] uppercase border-b border-[#333] bg-[#1a1a1a]">
                        当前 (existing)
                      </p>
                      <pre className="flex-1 overflow-auto p-3 text-xs text-[#888] whitespace-pre-wrap font-mono">
                        {result.previous[activeFile] ?? "(此文件之前不存在)"}
                      </pre>
                    </div>
                    {/* Proposed */}
                    <div className="flex flex-col min-h-0">
                      <p className="px-3 py-1.5 text-[10px] text-[#ff8c00] uppercase border-b border-[#333] bg-[#1a1a1a]">
                        提议 (dreaming proposal)
                      </p>
                      <textarea
                        value={editedFiles[activeFile] ?? ""}
                        onChange={(e) =>
                          setEditedFiles((prev) => ({
                            ...prev,
                            [activeFile]: e.target.value,
                          }))
                        }
                        className="flex-1 bg-[#1a1a1a] text-[#e5e5e5] text-xs p-3 font-mono resize-none focus:outline-none focus:bg-[#1e1e1e]"
                        placeholder="(留空 = 删除此文件)"
                      />
                    </div>
                  </div>
                </>
              )}
            </div>
          </div>
        )}

        {phase === "done" && !error && (
          <div className="flex-1 flex flex-col items-center justify-center px-8">
            <div className="text-4xl mb-4">✓</div>
            <p className="text-sm text-[#aaa]">记忆已更新。下次对话生效。</p>
          </div>
        )}

        {/* Footer */}
        {(phase === "review" || phase === "applying") && (
          <div className="flex justify-between items-center px-6 py-3 border-t border-[#333]">
            <p className="text-xs text-[#555]">
              已接受 {acceptedFiles.size}/{Object.keys(result?.proposal.files ?? {}).length}{" "}
              个文件
            </p>
            <div className="flex gap-3">
              <button
                onClick={onClose}
                className="px-4 py-2 text-sm text-[#888] hover:text-[#ccc] transition-colors"
              >
                取消
              </button>
              <button
                onClick={handleApply}
                disabled={acceptedFiles.size === 0}
                className="px-5 py-2 text-sm bg-[#ff8c00] text-white rounded-lg hover:bg-[#e07800] disabled:opacity-50 transition-colors font-medium"
              >
                {phase === "applying" ? "应用中…" : "应用记忆"}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
