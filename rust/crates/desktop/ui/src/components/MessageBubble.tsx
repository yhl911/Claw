import ReactMarkdown from "react-markdown";
import type { ToolCall } from "../hooks/useConversation";

interface Props {
  role: "user" | "assistant";
  text: string;
  inputTokens?: number;
  outputTokens?: number;
  toolCalls?: ToolCall[];
  inProgress?: boolean;
  iteration?: number;
}

const ROLE_BADGES: Record<string, string> = {
  "opc-product": "产品",
  "opc-engineering": "工程",
  "opc-finance": "财务",
  "opc-marketing": "市场",
  "opc-sales": "销售",
  "opc-ops": "运营",
  "opc-legal": "法务",
};

function ToolCallCard({ call, inProgress }: { call: ToolCall; inProgress: boolean }) {
  // Try to pull structured fields out of the input JSON for nicer display.
  let parsed: Record<string, unknown> = {};
  try {
    parsed = JSON.parse(call.inputPreview) as Record<string, unknown>;
  } catch {
    // partial JSON during streaming — fall back to raw preview
  }
  const subagentType = (parsed.subagent_type as string | undefined) ?? "";
  const description = (parsed.description as string | undefined) ?? "";
  const isAgent = call.toolName === "Agent";
  const badge = ROLE_BADGES[subagentType];

  return (
    <div className="my-2 rounded-lg border border-[#444] bg-[#1e1e1e] overflow-hidden">
      <div className="flex items-center gap-2 px-3 py-2 bg-[#252525] border-b border-[#333]">
        <span className="text-[#ff8c00] font-mono text-[11px] font-semibold uppercase tracking-wide">
          {isAgent ? "🚀 Agent" : `🔧 ${call.toolName}`}
        </span>
        {badge && (
          <span className="px-1.5 py-0.5 rounded text-[10px] bg-[#ff8c00]/20 text-[#ff8c00] font-medium">
            {badge}
          </span>
        )}
        {inProgress && (
          <span className="text-[10px] text-yellow-400 ml-auto animate-pulse">
            执行中…
          </span>
        )}
      </div>
      {description ? (
        <div className="px-3 py-2 text-xs text-[#ccc] leading-relaxed whitespace-pre-wrap">
          {description}
        </div>
      ) : (
        <pre className="px-3 py-2 text-[11px] text-[#888] font-mono whitespace-pre-wrap break-all overflow-x-auto">
          {call.inputPreview || "(streaming…)"}
        </pre>
      )}
    </div>
  );
}

export function MessageBubble({
  role,
  text,
  inputTokens,
  outputTokens,
  toolCalls,
  inProgress,
  iteration,
}: Props) {
  const isUser = role === "user";
  const hasTools = toolCalls && toolCalls.length > 0;

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"} mb-4`}>
      <div
        className={`max-w-[80%] rounded-2xl px-4 py-3 text-sm leading-relaxed ${
          isUser
            ? "bg-[#ff8c00] text-white rounded-br-sm"
            : "bg-[#2d2d2d] text-[#e5e5e5] rounded-bl-sm"
        }`}
      >
        {/* Iteration hint for multi-step assistant turns */}
        {!isUser && inProgress && iteration && iteration > 1 && (
          <div className="text-[10px] text-[#888] mb-1.5 uppercase tracking-wider">
            第 {iteration} 步 · 处理工具结果
          </div>
        )}

        {isUser ? (
          <p className="whitespace-pre-wrap m-0">{text}</p>
        ) : (
          <>
            {hasTools && (
              <div className="mb-2">
                {toolCalls!.map((call, i) => (
                  <ToolCallCard
                    key={call.toolUseId || i}
                    call={call}
                    inProgress={Boolean(inProgress)}
                  />
                ))}
              </div>
            )}
            <div className="prose prose-invert prose-sm max-w-none">
              <ReactMarkdown
                components={{
                  code({ children, className }) {
                    const isBlock = className?.includes("language-");
                    return isBlock ? (
                      <pre className="bg-[#1a1a1a] rounded-lg p-3 overflow-x-auto my-2">
                        <code className="text-[#e5e5e5] text-xs font-mono">
                          {children}
                        </code>
                      </pre>
                    ) : (
                      <code className="bg-[#1a1a1a] rounded px-1 py-0.5 text-[#ff8c00] text-xs font-mono">
                        {children}
                      </code>
                    );
                  },
                  p({ children }) {
                    return <p className="m-0 mb-2 last:mb-0">{children}</p>;
                  },
                }}
              >
                {text}
              </ReactMarkdown>
            </div>
            {/* Streaming cursor — only when actively in progress and showing text */}
            {inProgress && text.length > 0 && (
              <span className="inline-block w-1.5 h-4 align-text-bottom bg-[#ff8c00] animate-pulse ml-0.5" />
            )}
            {/* If still in progress with NO text and NO tool yet, show a small thinking marker */}
            {inProgress && !text && !hasTools && (
              <span className="text-xs text-[#666] italic">分析中…</span>
            )}
          </>
        )}
        {!isUser && !inProgress && (inputTokens || outputTokens) ? (
          <div className="mt-2 pt-2 border-t border-[#444] text-xs text-[#666] flex gap-3">
            {inputTokens ? <span>in: {inputTokens.toLocaleString()}</span> : null}
            {outputTokens ? <span>out: {outputTokens.toLocaleString()}</span> : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}
