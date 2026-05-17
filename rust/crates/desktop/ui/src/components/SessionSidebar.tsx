import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LongTaskPanel } from "./LongTaskPanel";
import { SkillsPanel } from "./SkillsPanel";
import { MCPPanel } from "./MCPPanel";
import { HooksPanel } from "./HooksPanel";
import { MemoryPanel } from "./MemoryPanel";
import { CostsPanel } from "./CostsPanel";
import { AnchorsPanel } from "./AnchorsPanel";

type NavPanel = "skills" | "mcp" | "hooks" | "memory" | "costs" | "anchors" | null;

interface SessionInfo {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  message_count: number;
}

interface TokenStats {
  today_input: number;
  today_output: number;
  week_input: number;
  week_output: number;
  month_input: number;
  month_output: number;
  total_input: number;
  total_output: number;
  turn_count_today: number;
  month_cost_usd: number;
}

function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1000_000) return `${(n / 1000).toFixed(1)}K`;
  return `${(n / 1000_000).toFixed(2)}M`;
}

interface Props {
  /// Called when the user picks a session OR creates a new one. Parent
  /// uses this to clear local UI state and re-trigger restore_session.
  onSwitched: () => void;
  /// Bumped by parent (ChatPanel) when a new long task was just started
  /// so the embedded `LongTaskPanel` refreshes immediately.
  longTaskRefresh?: number;
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

export function SessionSidebar({ onSwitched, longTaskRefresh }: Props) {
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [currentId, setCurrentId] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [stats, setStats] = useState<TokenStats | null>(null);
  const [statsExpanded, setStatsExpanded] = useState(false);
  const [openPanel, setOpenPanel] = useState<NavPanel>(null);

  async function refresh() {
    try {
      const [list, cur, st] = await Promise.all([
        invoke<SessionInfo[]>("list_sessions"),
        invoke<string>("current_session_id"),
        invoke<TokenStats>("get_token_stats"),
      ]);
      setSessions(list);
      setCurrentId(cur);
      setStats(st);
    } catch (e) {
      console.warn("[SessionSidebar] refresh failed:", e);
    }
  }

  useEffect(() => {
    refresh();
    // Refresh when backend signals session boundary changes
    const off = listen<string>("session-changed", (e) => {
      setCurrentId(e.payload);
      refresh();
    });
    // Also poll every 5s so the title for the active session updates as
    // new messages roll in (title is derived from first user message;
    // it stabilizes after the first send and rarely changes after).
    const timer = setInterval(refresh, 5000);
    return () => {
      off.then((f) => f());
      clearInterval(timer);
    };
  }, []);

  async function handleSwitch(id: string) {
    if (id === currentId || busy) return;
    setBusy(true);
    try {
      await invoke("switch_session", { sessionId: id });
      setCurrentId(id);
      onSwitched();
      await refresh();
    } catch (e) {
      console.error("[SessionSidebar] switch failed:", e);
    } finally {
      setBusy(false);
    }
  }

  async function handleNew() {
    if (busy) return;
    setBusy(true);
    try {
      const newId = await invoke<string>("new_session");
      setCurrentId(newId);
      onSwitched();
      await refresh();
    } catch (e) {
      console.error("[SessionSidebar] new failed:", e);
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete(id: string, e: React.MouseEvent) {
    e.stopPropagation();
    if (id === currentId) return;
    if (!confirm("删除这个会话？此操作不可撤销。")) return;
    try {
      await invoke("delete_session", { sessionId: id });
      await refresh();
    } catch (err) {
      console.error("[SessionSidebar] delete failed:", err);
    }
  }

  const navItems: { key: NavPanel; label: string; icon: string }[] = [
    { key: "skills", label: "Skills", icon: "🧰" },
    { key: "mcp", label: "MCP Servers", icon: "🧩" },
    { key: "hooks", label: "Hooks", icon: "🔔" },
    { key: "memory", label: "Memory", icon: "🧠" },
    { key: "anchors", label: "Anchors", icon: "📌" },
    { key: "costs", label: "Costs", icon: "💰" },
  ];

  return (
    <div className="w-56 border-r border-[#333] bg-[#181818] flex flex-col flex-shrink-0">
      <div className="px-2 py-2 border-b border-[#333] space-y-0.5">
        <button
          onClick={handleNew}
          disabled={busy}
          className="w-full flex items-center gap-2 px-2 py-1.5 rounded text-xs text-[#e5e5e5] hover:bg-[#252525] disabled:opacity-30 transition-colors"
          title="新建会话"
        >
          <span className="text-base leading-none">+</span>
          <span>新建会话</span>
        </button>
        {navItems.map((item) => (
          <button
            key={item.key}
            onClick={() => setOpenPanel(item.key)}
            className="w-full flex items-center gap-2 px-2 py-1.5 rounded text-xs text-[#bbb] hover:bg-[#252525] hover:text-[#e5e5e5] transition-colors"
          >
            <span className="text-sm leading-none">{item.icon}</span>
            <span>{item.label}</span>
          </button>
        ))}
      </div>

      <div className="px-3 pt-3 pb-1">
        <h2 className="text-[10px] font-semibold text-[#666] uppercase tracking-wider">
          Recents
        </h2>
      </div>

      <div className="flex-1 overflow-y-auto p-1.5 pt-0">
        {sessions.length === 0 ? (
          <p className="text-xs text-[#555] text-center mt-6 px-2">
            还没有会话历史
          </p>
        ) : (
          sessions.map((s) => {
            const isCurrent = s.id === currentId;
            return (
              <div
                key={s.id}
                onClick={() => handleSwitch(s.id)}
                className={`group mb-1 px-2 py-2 rounded cursor-pointer transition-colors ${
                  isCurrent
                    ? "bg-[#2d2d2d] border border-[#ff8c00]/40"
                    : "hover:bg-[#252525] border border-transparent"
                }`}
              >
                <div className="flex items-start justify-between gap-1">
                  <p
                    className={`text-xs leading-snug line-clamp-2 flex-1 ${
                      isCurrent
                        ? "text-[#e5e5e5] font-medium"
                        : "text-[#bbb]"
                    }`}
                  >
                    {s.title}
                  </p>
                  {!isCurrent && (
                    <button
                      onClick={(e) => handleDelete(s.id, e)}
                      title="删除会话"
                      className="opacity-0 group-hover:opacity-100 text-[#666] hover:text-red-400 text-[10px] flex-shrink-0 transition-opacity"
                    >
                      ✕
                    </button>
                  )}
                </div>
                <div className="flex items-center justify-between mt-1 text-[10px] text-[#666]">
                  <span>{relativeTime(s.updated_at)}</span>
                  <span>{s.message_count} 条</span>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Long-running tasks live between sessions and token stats. Renders
          nothing when there are no tasks. */}
      <LongTaskPanel refreshTrigger={longTaskRefresh} />

      {/* Token stats footer — collapsed shows today's totals; expanded
          shows week/month + cost estimate. */}
      {stats && (stats.today_input + stats.today_output > 0) && (
        <div
          onClick={() => setStatsExpanded((v) => !v)}
          className="px-3 py-2 border-t border-[#333] bg-[#141414] text-[10px] text-[#888] cursor-pointer hover:bg-[#181818] transition-colors"
          title="点击展开/收起 token 统计"
        >
          <div className="flex items-center justify-between">
            <span className="font-medium text-[#aaa]">📊 今日</span>
            <span>
              ↓{formatTokens(stats.today_input)} ↑{formatTokens(stats.today_output)}{" "}
              · {stats.turn_count_today} 轮
            </span>
          </div>
          {statsExpanded && (
            <div className="mt-1.5 pt-1.5 border-t border-[#222] space-y-0.5">
              <div className="flex items-center justify-between">
                <span>本周</span>
                <span>
                  ↓{formatTokens(stats.week_input)} ↑{formatTokens(stats.week_output)}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span>本月</span>
                <span>
                  ↓{formatTokens(stats.month_input)} ↑{formatTokens(stats.month_output)}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span>累计</span>
                <span>
                  ↓{formatTokens(stats.total_input)} ↑{formatTokens(stats.total_output)}
                </span>
              </div>
              <div className="flex items-center justify-between pt-0.5 border-t border-[#222] mt-1">
                <span className="text-[#aaa] font-medium">本月费用 (估算)</span>
                <span className="text-[#ff8c00] font-mono">
                  ${stats.month_cost_usd.toFixed(3)}
                </span>
              </div>
            </div>
          )}
        </div>
      )}

      {openPanel === "skills" && <SkillsPanel onClose={() => setOpenPanel(null)} />}
      {openPanel === "mcp" && (
        <MCPPanel
          onClose={() => setOpenPanel(null)}
          onSaved={() => setOpenPanel(null)}
        />
      )}
      {openPanel === "hooks" && (
        <HooksPanel
          onClose={() => setOpenPanel(null)}
          onSaved={() => setOpenPanel(null)}
        />
      )}
      {openPanel === "memory" && <MemoryPanel onClose={() => setOpenPanel(null)} />}
      {openPanel === "anchors" && <AnchorsPanel onClose={() => setOpenPanel(null)} />}
      {openPanel === "costs" && <CostsPanel onClose={() => setOpenPanel(null)} />}
    </div>
  );
}
