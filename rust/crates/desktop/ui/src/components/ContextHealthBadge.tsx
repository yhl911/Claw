import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

interface ContextHealthPayload {
  input_tokens: number;
  window: number;
  fill_ratio: number;
  model: string;
}

/**
 * Live indicator of how full the current session's context window is.
 *
 * Why this exists: Claude (and most LLMs) start losing accuracy long
 * before the hard token limit — research and user reports put the
 * degradation curve at roughly 20% fill, with "Lost in the Middle"
 * effects compounding past 40%. Users have no way to feel this without
 * a number, so they let sessions grow until output quality collapses.
 *
 * Color thresholds:
 *   <  40%  green  — comfortable
 *   40-70%  amber  — consider /dream or summarize
 *   >= 70%  red    — strongly suggest restart / compaction
 */
export function ContextHealthBadge() {
  const [health, setHealth] = useState<ContextHealthPayload | null>(null);

  useEffect(() => {
    const off = listen<ContextHealthPayload>("context-health", (e) => {
      setHealth(e.payload);
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  if (!health) return null;

  const pct = Math.round(health.fill_ratio * 100);
  let color = "text-[#888]";
  let bg = "bg-[#2a2a2a]";
  let title = "上下文使用率（越低模型记忆越准）";
  if (pct >= 70) {
    color = "text-red-400";
    bg = "bg-red-900/30";
    title = "⚠️ 上下文已接近极限 — 强烈建议清空会话或运行 /dream 压缩历史";
  } else if (pct >= 40) {
    color = "text-amber-400";
    bg = "bg-amber-900/20";
    title = "⚠️ 上下文超过 40%，模型开始出现 Lost-in-Middle 现象。建议尽快压缩。";
  }

  const tokensLabel = formatTokens(health.input_tokens);
  const windowLabel = formatTokens(health.window);

  return (
    <div
      title={title}
      className={`flex items-center gap-1.5 px-2 py-0.5 rounded ${bg} ${color} text-[11px] font-mono`}
    >
      <span className="opacity-60">ctx</span>
      <span>{pct}%</span>
      <span className="opacity-40">·</span>
      <span className="opacity-70">
        {tokensLabel}/{windowLabel}
      </span>
    </div>
  );
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}k`;
  return `${n}`;
}
