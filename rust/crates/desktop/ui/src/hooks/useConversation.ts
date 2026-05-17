import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

export interface ToolCall {
  toolUseId: string;
  toolName: string;
  inputPreview: string;
}

export interface Message {
  id: string;
  role: "user" | "assistant";
  text: string;
  inputTokens?: number;
  outputTokens?: number;
  /// Tool calls observed during this assistant turn (rendered as inline cards).
  toolCalls?: ToolCall[];
  /// While true, this message is still being streamed in.
  inProgress?: boolean;
  /// Iteration counter — increments each time the model takes another turn
  /// after a tool result. Useful for "step 1/2/3" UI hints.
  iteration?: number;
}

interface TurnResult {
  text: string;
  input_tokens: number;
  output_tokens: number;
}

type TurnStreamPayload =
  | { kind: "text-delta"; text: string }
  | { kind: "tool-start"; tool_use_id: string; tool_name: string; input_preview: string }
  | { kind: "iteration"; n: number };

interface RestoredMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
}

export function useConversation(sessionEpoch: number = 0) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [thinking, setThinking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [restoredCount, setRestoredCount] = useState(0);
  const counterRef = useRef(0);
  /// ID of the currently streaming assistant message (so deltas can find it).
  const liveIdRef = useRef<string | null>(null);

  // Restore conversation on mount AND on every session switch
  // (sessionEpoch bump). When the user picks a different session in
  // the sidebar, parent bumps sessionEpoch → this effect re-runs →
  // we wipe local UI state and reload from the new session's jsonl.
  useEffect(() => {
    let live = true;
    // Reset UI state immediately so the user sees the switch reflected
    // even before restore_session resolves.
    setMessages([]);
    setError(null);
    setRestoredCount(0);
    liveIdRef.current = null;
    counterRef.current = 0;

    invoke<RestoredMessage[]>("restore_session")
      .then((restored) => {
        if (!live) return;
        if (restored.length === 0) return;
        setMessages(
          restored.map((m) => ({ id: m.id, role: m.role, text: m.text })),
        );
        counterRef.current = restored.length + 1;
        setRestoredCount(restored.length);
      })
      .catch((e) => console.warn("[useConversation] restore failed:", e));
    return () => {
      live = false;
    };
  }, [sessionEpoch]);

  // Subscribe to streaming events from the backend.
  useEffect(() => {
    const offTurnStart = listen("turn-start", () => {
      const id = String(counterRef.current++);
      liveIdRef.current = id;
      setMessages((prev) => [
        ...prev,
        { id, role: "assistant", text: "", inProgress: true, toolCalls: [], iteration: 1 },
      ]);
    });

    const offStream = listen<TurnStreamPayload>("turn-stream", (e) => {
      const payload = e.payload;
      const targetId = liveIdRef.current;
      if (!targetId) return;

      if (payload.kind === "text-delta") {
        setMessages((prev) =>
          prev.map((m) =>
            m.id === targetId ? { ...m, text: m.text + payload.text } : m,
          ),
        );
      } else if (payload.kind === "tool-start") {
        const call: ToolCall = {
          toolUseId: payload.tool_use_id,
          toolName: payload.tool_name,
          inputPreview: payload.input_preview,
        };
        setMessages((prev) =>
          prev.map((m) =>
            m.id === targetId
              ? { ...m, toolCalls: [...(m.toolCalls ?? []), call] }
              : m,
          ),
        );
      } else if (payload.kind === "iteration") {
        setMessages((prev) =>
          prev.map((m) =>
            m.id === targetId ? { ...m, iteration: payload.n } : m,
          ),
        );
      }
    });

    return () => {
      offTurnStart.then((f) => f());
      offStream.then((f) => f());
    };
  }, []);

  async function sendMessage(text: string) {
    if (!text.trim() || thinking) return;

    const userId = String(counterRef.current++);
    setMessages((prev) => [...prev, { id: userId, role: "user", text }]);
    setThinking(true);
    setError(null);

    try {
      const result = await invoke<TurnResult>("send_message", { message: text });
      const liveId = liveIdRef.current;
      // Finalize the streamed bubble: stamp tokens, mark done.
      // If for some reason the live bubble doesn't exist (no turn-start received),
      // fall back to creating a new one with the final text.
      setMessages((prev) => {
        if (liveId && prev.some((m) => m.id === liveId)) {
          return prev.map((m) =>
            m.id === liveId
              ? {
                  ...m,
                  // Prefer the streamed text we accumulated; only fall back to
                  // the final result text when nothing was streamed (rare).
                  text: m.text || result.text,
                  inputTokens: result.input_tokens,
                  outputTokens: result.output_tokens,
                  inProgress: false,
                }
              : m,
          );
        }
        const id = String(counterRef.current++);
        return [
          ...prev,
          {
            id,
            role: "assistant",
            text: result.text,
            inputTokens: result.input_tokens,
            outputTokens: result.output_tokens,
          },
        ];
      });
      liveIdRef.current = null;
      setError(null);
    } catch (e) {
      const msg = String(e);
      const liveId = liveIdRef.current;
      if (msg.includes("__CANCELLED__")) {
        // Mark the live bubble as cancelled but keep what was streamed so far.
        setMessages((prev) =>
          prev.map((m) =>
            m.id === liveId
              ? { ...m, text: (m.text || "_(已中止)_"), inProgress: false }
              : m,
          ),
        );
      } else {
        // Drop the empty in-progress bubble if nothing streamed; keep otherwise.
        setMessages((prev) =>
          prev.flatMap((m) => {
            if (m.id !== liveId) return [m];
            if (!m.text && (!m.toolCalls || m.toolCalls.length === 0)) return [];
            return [{ ...m, inProgress: false }];
          }),
        );
        setError(msg);
      }
      liveIdRef.current = null;
    } finally {
      setThinking(false);
    }
  }

  async function cancelTurn() {
    try {
      await invoke("cancel_turn");
    } catch (e) {
      console.error("[useConversation] cancel failed:", e);
    }
  }

  async function clearSession() {
    try {
      await invoke("clear_session");
      setMessages([]);
      setError(null);
      liveIdRef.current = null;
      setRestoredCount(0);
    } catch (e) {
      setError(String(e));
    }
  }

  return {
    messages,
    thinking,
    error,
    sendMessage,
    cancelTurn,
    clearSession,
    restoredCount,
  };
}
