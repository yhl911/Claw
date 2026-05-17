import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentDetailModal } from "./AgentDetailModal";
import { AgentProfileModal } from "./AgentProfileModal";

interface OpcAgentInfo {
  id: string;
  subagent_type: string;
  status: string;
  description: string;
  created_at_secs: number;
}

interface Props {
  /// Called when the user clicks the "summarize" shortcut. Parent injects
  /// the prompt into the chat input.
  onSummarize?: (prompt: string) => void;
}

const STATUS_COLORS: Record<string, string> = {
  running: "bg-yellow-500 animate-pulse",
  completed: "bg-green-500",
  failed: "bg-red-500",
};

const ROLE_LABELS: Record<string, string> = {
  "opc-product": "产品",
  "opc-engineering": "工程",
  "opc-finance": "财务",
  "opc-marketing": "市场",
  "opc-sales": "销售",
  "opc-ops": "运营",
  "opc-legal": "法务",
};

/// Two manifests within this many seconds of each other are considered the
/// same "turn" (one CEO assistant message that fanned out multiple Agent
/// calls). Any larger gap = a new turn boundary.
const TURN_GROUP_WINDOW_SECS = 90;

interface TurnGroup {
  turn_id: string; // earliest createdAt in the group
  earliest: number;
  latest: number;
  agents: OpcAgentInfo[];
}

function groupByTurn(agents: OpcAgentInfo[]): TurnGroup[] {
  // agents already arrive newest-first from the backend
  const sorted = [...agents].sort(
    (a, b) => b.created_at_secs - a.created_at_secs,
  );
  const groups: TurnGroup[] = [];
  for (const agent of sorted) {
    const head = groups[groups.length - 1];
    if (
      head &&
      Math.abs(head.earliest - agent.created_at_secs) <= TURN_GROUP_WINDOW_SECS
    ) {
      head.agents.push(agent);
      head.earliest = Math.min(head.earliest, agent.created_at_secs);
      head.latest = Math.max(head.latest, agent.created_at_secs);
    } else {
      groups.push({
        turn_id: String(agent.created_at_secs),
        earliest: agent.created_at_secs,
        latest: agent.created_at_secs,
        agents: [agent],
      });
    }
  }
  return groups;
}

function relativeTime(unixSecs: number): string {
  if (!unixSecs) return "";
  const now = Math.floor(Date.now() / 1000);
  const diff = now - unixSecs;
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

export function OpcAgentPanel({ onSummarize }: Props) {
  const [agents, setAgents] = useState<OpcAgentInfo[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [profileRole, setProfileRole] = useState<string | null>(null);
  const [showProfileMenu, setShowProfileMenu] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const refresh = () => {
      invoke<OpcAgentInfo[]>("list_opc_agents")
        .then(setAgents)
        .catch(() => {});
    };
    refresh();
    const timer = setInterval(refresh, 2000);
    return () => clearInterval(timer);
  }, []);

  const groups = useMemo(() => groupByTurn(agents), [agents]);
  const runningCount = agents.filter((a) => a.status === "running").length;
  const completedCount = agents.filter((a) => a.status === "completed").length;
  const failedCount = agents.filter((a) => a.status === "failed").length;
  const terminalCount = completedCount + failedCount;

  async function handleClear(scope: "terminal" | "all") {
    setBusy(true);
    try {
      const statuses =
        scope === "terminal"
          ? ["completed", "failed"]
          : ["completed", "failed", "running"];
      await invoke<number>("clear_agents", { statuses });
      // Optimistically refresh
      const fresh = await invoke<OpcAgentInfo[]>("list_opc_agents");
      setAgents(fresh);
    } catch (e) {
      console.error("[clear_agents]", e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="w-64 border-l border-[#333] bg-[#1e1e1e] flex flex-col flex-shrink-0">
        <div className="px-4 py-3 border-b border-[#333] relative">
          <div className="flex items-center justify-between">
            <h2 className="text-xs font-semibold text-[#888] uppercase tracking-wider">
              OPC Agents
            </h2>
            <div className="flex items-center gap-2">
              <button
                onClick={() => setShowProfileMenu((v) => !v)}
                title="生成角色画像"
                className="text-[10px] text-[#666] hover:text-[#ff8c00] transition-colors"
              >
                📋 画像
              </button>
              <span className="text-xs text-[#555]">{agents.length}</span>
            </div>
          </div>

          {showProfileMenu && (
            <div className="absolute top-full right-2 mt-1 z-10 bg-[#2d2d2d] border border-[#444] rounded-lg shadow-xl py-1 min-w-[120px]">
              {Object.entries(ROLE_LABELS).map(([role, label]) => (
                <button
                  key={role}
                  onClick={() => {
                    setProfileRole(role);
                    setShowProfileMenu(false);
                  }}
                  className="w-full px-3 py-1.5 text-left text-xs text-[#ddd] hover:bg-[#3a3a3a] transition-colors"
                >
                  {label}
                  <span className="ml-2 text-[#666]">{role}</span>
                </button>
              ))}
            </div>
          )}

          {agents.length > 0 && (
            <div className="flex items-center justify-between mt-1.5 text-[10px]">
              <div className="flex gap-2">
                {runningCount > 0 && (
                  <span className="text-yellow-400">● {runningCount}</span>
                )}
                {completedCount > 0 && (
                  <span className="text-green-400">✓ {completedCount}</span>
                )}
                {failedCount > 0 && (
                  <span className="text-red-400">✗ {failedCount}</span>
                )}
              </div>
              {terminalCount > 0 && (
                <button
                  onClick={() => handleClear("terminal")}
                  disabled={busy}
                  title={`清除已完成与已失败 (${terminalCount})`}
                  className="text-[#666] hover:text-[#ff8c00] disabled:opacity-30 transition-colors"
                >
                  🗑 清{terminalCount}
                </button>
              )}
            </div>
          )}
        </div>

        <div className="flex-1 overflow-y-auto p-2">
          {agents.length === 0 ? (
            <p className="text-xs text-[#555] text-center mt-6 px-2 leading-relaxed">
              没有活跃的 OPC Agent。
              <br />
              <span className="text-[#444]">CEO 委派任务后将在此显示。</span>
            </p>
          ) : (
            groups.map((group, gi) => (
              <div key={group.turn_id} className="mb-3">
                {/* Turn separator — only show if there's more than one group
                    or if this group has multiple agents */}
                {(groups.length > 1 || group.agents.length > 1) && (
                  <div className="flex items-center gap-2 px-1 mb-1.5">
                    <span className="text-[10px] text-[#666] font-medium">
                      {gi === 0 ? "当前轮" : `第 ${groups.length - gi} 轮`}
                    </span>
                    <span className="text-[10px] text-[#555]">·</span>
                    <span className="text-[10px] text-[#555]">
                      {relativeTime(group.latest)}
                    </span>
                    <span className="text-[10px] text-[#555]">·</span>
                    <span className="text-[10px] text-[#555]">
                      {group.agents.length}
                    </span>
                    <div className="flex-1 h-px bg-[#2d2d2d]" />
                  </div>
                )}
                {group.agents.map((agent) => (
                  <button
                    key={agent.id}
                    onClick={() => setSelectedId(agent.id)}
                    className="w-full text-left mb-1.5 p-2 rounded-lg bg-[#242424] border border-[#333] hover:border-[#ff8c00] hover:bg-[#2a2a2a] transition-colors"
                  >
                    <div className="flex items-center gap-2 mb-1">
                      <span
                        className={`w-2 h-2 rounded-full flex-shrink-0 ${
                          STATUS_COLORS[agent.status] ?? "bg-gray-500"
                        }`}
                      />
                      <span className="text-xs font-medium text-[#e5e5e5]">
                        {ROLE_LABELS[agent.subagent_type] ?? agent.subagent_type}
                      </span>
                    </div>
                    {agent.description ? (
                      <p className="text-xs text-[#888] leading-snug line-clamp-2">
                        {agent.description}
                      </p>
                    ) : null}
                    <span className="text-[10px] text-[#555] mt-1 block">
                      {agent.status}
                    </span>
                  </button>
                ))}
              </div>
            ))
          )}
        </div>

        {/* Footer: summarize shortcut + clear-all (when stuff is running we
            don't show clear-all since it would orphan running agents). */}
        {agents.length > 0 && (
          <div className="px-3 py-2 border-t border-[#333] bg-[#1a1a1a] space-y-1.5">
            {onSummarize && terminalCount > 0 && runningCount === 0 && (
              <button
                onClick={() =>
                  onSummarize(
                    "请基于这次会话已完成的所有 sub-agent 输出，做一次综合汇报。",
                  )
                }
                className="w-full px-3 py-1.5 text-xs bg-[#ff8c00]/10 hover:bg-[#ff8c00]/20 border border-[#ff8c00]/30 hover:border-[#ff8c00] rounded text-[#ff8c00] transition-colors flex items-center justify-center gap-1.5"
                title="让 CEO 综合所有 sub-agent 的结果"
              >
                <span>📋</span>
                <span>让 CEO 综合汇报</span>
              </button>
            )}
            {agents.length >= 5 && (
              <button
                onClick={() => handleClear("all")}
                disabled={busy || runningCount > 0}
                title={
                  runningCount > 0
                    ? "等运行中的 agent 完成后再清"
                    : "清除全部 agent 记录（不影响运行中的线程）"
                }
                className="w-full px-3 py-1 text-[10px] text-[#666] hover:text-red-400 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
              >
                🗑 清除全部 ({agents.length})
              </button>
            )}
          </div>
        )}
      </div>

      {selectedId && (
        <AgentDetailModal agentId={selectedId} onClose={() => setSelectedId(null)} />
      )}

      {profileRole && (
        <AgentProfileModal role={profileRole} onClose={() => setProfileRole(null)} />
      )}
    </>
  );
}
