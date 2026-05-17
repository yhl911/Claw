import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface ModelBreakdown {
  model: string;
  input_tokens: number;
  output_tokens: number;
  turns: number;
  cost_usd: number;
}

interface DailyCost {
  date: string;
  cost_usd: number;
  turns: number;
}

interface CostBreakdown {
  window: string;
  total_cost_usd: number;
  total_turns: number;
  by_model: ModelBreakdown[];
  daily_history: DailyCost[];
}

interface ModelQuality {
  model: string;
  turns: number;
  avg_iterations: number;
  tool_error_rate: number;
  max_iterations: number;
  telemetry_turns: number;
}

interface QualityBreakdown {
  window: string;
  by_model: ModelQuality[];
}

interface Props {
  onClose: () => void;
}

type Tab = "cost" | "quality";

const WINDOWS: { key: string; label: string; secs: number }[] = [
  { key: "day", label: "今日", secs: 86_400 },
  { key: "week", label: "本周", secs: 7 * 86_400 },
  { key: "month", label: "本月", secs: 30 * 86_400 },
];

function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}K`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

export function CostsPanel({ onClose }: Props) {
  const [window, setWindow] = useState<number>(30 * 86_400);
  const [data, setData] = useState<CostBreakdown | null>(null);
  const [quality, setQuality] = useState<QualityBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [tab, setTab] = useState<Tab>("cost");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    Promise.all([
      invoke<CostBreakdown>("get_cost_breakdown", { windowSecs: window }),
      invoke<QualityBreakdown>("get_quality_breakdown", { windowSecs: window }),
    ])
      .then(([cost, qual]) => {
        if (!cancelled) {
          setData(cost);
          setQuality(qual);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [window]);

  const sparkMax = useMemo(() => {
    if (!data) return 0;
    return Math.max(0.0001, ...data.daily_history.map((d) => d.cost_usd));
  }, [data]);

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
          <div className="flex items-center gap-3">
            <span className="text-lg">💰</span>
            <div className="flex gap-1">
              <button
                onClick={() => setTab("cost")}
                className={`text-xs px-3 py-1 rounded transition-colors ${
                  tab === "cost"
                    ? "bg-[#2d2d2d] text-[#e5e5e5]"
                    : "text-[#888] hover:text-[#ccc]"
                }`}
              >
                成本
              </button>
              <button
                onClick={() => setTab("quality")}
                className={`text-xs px-3 py-1 rounded transition-colors ${
                  tab === "quality"
                    ? "bg-[#2d2d2d] text-[#e5e5e5]"
                    : "text-[#888] hover:text-[#ccc]"
                }`}
              >
                质量
              </button>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-[#888] hover:text-[#e5e5e5] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-4">
          {/* Window selector */}
          <div className="flex gap-1">
            {WINDOWS.map((w) => (
              <button
                key={w.key}
                onClick={() => setWindow(w.secs)}
                className={`text-xs px-3 py-1 rounded transition-colors ${
                  window === w.secs
                    ? "bg-[#ff8c00] text-black"
                    : "bg-[#222] text-[#aaa] hover:bg-[#2d2d2d]"
                }`}
              >
                {w.label}
              </button>
            ))}
          </div>

          {error && (
            <div className="px-3 py-2 bg-red-900/30 border border-red-800 rounded text-xs text-red-300">
              {error}
            </div>
          )}

          {loading && !data && (
            <p className="text-xs text-[#666] py-4 text-center">加载中…</p>
          )}

          {tab === "cost" && data && (
            <>
              {/* Summary */}
              <div className="grid grid-cols-2 gap-3">
                <div className="bg-[#222] border border-[#333] rounded p-3">
                  <div className="text-xs text-[#888]">总成本</div>
                  <div className="text-2xl font-mono text-[#ff8c00] mt-0.5">
                    ${data.total_cost_usd.toFixed(3)}
                  </div>
                </div>
                <div className="bg-[#222] border border-[#333] rounded p-3">
                  <div className="text-xs text-[#888]">对话轮次</div>
                  <div className="text-2xl font-mono text-[#e5e5e5] mt-0.5">
                    {data.total_turns}
                  </div>
                </div>
              </div>

              {/* 14-day sparkline */}
              <div>
                <div className="text-xs text-[#888] mb-1">最近 14 天</div>
                <div className="flex items-end gap-0.5 h-16 bg-[#0d0d0d] border border-[#2a2a2a] rounded p-1">
                  {data.daily_history.map((d) => {
                    const h = Math.max(2, (d.cost_usd / sparkMax) * 56);
                    return (
                      <div
                        key={d.date}
                        title={`${d.date} · $${d.cost_usd.toFixed(3)} · ${d.turns} 轮`}
                        className="flex-1 bg-[#ff8c00]/80 hover:bg-[#ff8c00] transition-colors cursor-help rounded-sm"
                        style={{ height: `${h}px` }}
                      />
                    );
                  })}
                </div>
                <div className="flex justify-between text-[10px] text-[#555] mt-1">
                  <span>{data.daily_history[0]?.date}</span>
                  <span>
                    {data.daily_history[data.daily_history.length - 1]?.date}
                  </span>
                </div>
              </div>

              {/* Per-model cost table */}
              <div>
                <div className="text-xs text-[#888] mb-1.5">按模型分组</div>
                {data.by_model.length === 0 ? (
                  <p className="text-xs text-[#555] py-4 text-center">
                    所选时间段没有调用记录
                  </p>
                ) : (
                  <div className="bg-[#0d0d0d] border border-[#2a2a2a] rounded overflow-hidden">
                    <div className="grid grid-cols-[1fr_auto_auto_auto] text-[10px] text-[#666] px-2 py-1 bg-[#1a1a1a] border-b border-[#2a2a2a]">
                      <div>模型</div>
                      <div className="text-right pr-3">输入 / 输出</div>
                      <div className="text-right pr-3">轮次</div>
                      <div className="text-right">成本</div>
                    </div>
                    {data.by_model.map((m) => (
                      <div
                        key={m.model}
                        className="grid grid-cols-[1fr_auto_auto_auto] text-xs px-2 py-1.5 border-b border-[#1a1a1a] last:border-b-0"
                      >
                        <div className="text-[#ccc] font-mono truncate">
                          {m.model}
                        </div>
                        <div className="text-right text-[#888] pr-3 font-mono">
                          {fmtTokens(m.input_tokens)} / {fmtTokens(m.output_tokens)}
                        </div>
                        <div className="text-right text-[#aaa] pr-3">{m.turns}</div>
                        <div className="text-right text-[#ff8c00] font-mono">
                          ${m.cost_usd.toFixed(3)}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </>
          )}

          {tab === "quality" && quality && (
            <>
              <p className="text-xs text-[#666] leading-relaxed">
                两个核心质量指标：<br />
                <span className="text-[#aaa]">• 平均迭代数</span> — 每轮模型用了多少次工具调用 / assistant 消息才收敛到答复。越低越说明模型直奔目标。<br />
                <span className="text-[#aaa]">• 工具错误率</span> — 这轮里有多少比例的工具调用返回了错误。包括 Loop Detector 触发的循环错误。
              </p>

              {quality.by_model.length === 0 ? (
                <p className="text-xs text-[#555] py-6 text-center">
                  所选时间段没有质量数据（telemetry 是最近版本才开始记录）
                </p>
              ) : (
                <div className="bg-[#0d0d0d] border border-[#2a2a2a] rounded overflow-hidden">
                  <div className="grid grid-cols-[1fr_auto_auto_auto_auto] text-[10px] text-[#666] px-2 py-1 bg-[#1a1a1a] border-b border-[#2a2a2a]">
                    <div>模型</div>
                    <div className="text-right pr-3">轮次</div>
                    <div className="text-right pr-3">平均迭代</div>
                    <div className="text-right pr-3">峰值</div>
                    <div className="text-right">工具错误率</div>
                  </div>
                  {quality.by_model.map((m) => {
                    const iter = m.avg_iterations;
                    const iterColor =
                      iter === 0
                        ? "text-[#666]"
                        : iter < 3
                          ? "text-green-400"
                          : iter < 6
                            ? "text-amber-400"
                            : "text-red-400";
                    const errPct = (m.tool_error_rate * 100).toFixed(0);
                    const errColor =
                      m.tool_error_rate === 0
                        ? "text-green-400"
                        : m.tool_error_rate < 0.15
                          ? "text-amber-400"
                          : "text-red-400";
                    return (
                      <div
                        key={m.model}
                        className="grid grid-cols-[1fr_auto_auto_auto_auto] text-xs px-2 py-1.5 border-b border-[#1a1a1a] last:border-b-0"
                        title={
                          m.telemetry_turns < m.turns
                            ? `${m.telemetry_turns}/${m.turns} 轮有 telemetry`
                            : ""
                        }
                      >
                        <div className="text-[#ccc] font-mono truncate">
                          {m.model}
                        </div>
                        <div className="text-right text-[#aaa] pr-3">
                          {m.turns}
                        </div>
                        <div
                          className={`text-right ${iterColor} pr-3 font-mono`}
                        >
                          {iter === 0 ? "—" : iter.toFixed(1)}
                        </div>
                        <div className="text-right text-[#888] pr-3 font-mono">
                          {m.max_iterations || "—"}
                        </div>
                        <div className={`text-right ${errColor} font-mono`}>
                          {m.tool_error_rate === 0 && m.turns === 0
                            ? "—"
                            : `${errPct}%`}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}

              <p className="text-[10px] text-[#555] leading-snug">
                ℹ️ telemetry 记录是版本兼容的：升级前的旧 turn 不会有迭代数/工具错误数据，
                那些行会显示 "—"。
              </p>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
