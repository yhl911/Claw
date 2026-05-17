import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { MessageBubble } from "./MessageBubble";
import { SettingsModal } from "./SettingsModal";
import { DreamReviewModal } from "./DreamReviewModal";
import { PlusMenu } from "./PlusMenu";
import { ContextHealthBadge } from "./ContextHealthBadge";
import { useConversation } from "../hooks/useConversation";

interface AttachedFile {
  name: string;
  content: string;
}

interface PastedImage {
  /** Preview data URL for display only (includes data: prefix). */
  preview: string;
  /** Raw base64 bytes — no data: prefix, sent to backend. */
  data: string;
  media_type: string;
}

interface DreamPendingPayload {
  proposal: { files: Record<string, string>; rationale: string };
  previous: Record<string, string>;
}

interface ChatPanelProps {
  /// Externally injected prompt (e.g. from OpcAgentPanel's "汇总结果" button).
  /// Each emit has a fresh `id` so identical strings still propagate.
  queuedInput?: { id: number; text: string } | null;
  /// Bumped by parent on every session switch so the conversation hook
  /// can reset and reload from the new session's persisted messages.
  sessionEpoch?: number;
  /// Called when a long-running task was just created. Parent uses this
  /// to bump `longTaskRefresh` on the sidebar so the new task appears.
  onLongTaskStarted?: () => void;
}

export function ChatPanel({ queuedInput, sessionEpoch, onLongTaskStarted }: ChatPanelProps) {
  const {
    messages,
    thinking,
    error,
    sendMessage,
    cancelTurn,
    clearSession,
    restoredCount,
  } = useConversation(sessionEpoch);
  const [input, setInput] = useState("");
  const [showSettings, setShowSettings] = useState(false);
  const [showDream, setShowDream] = useState(false);
  const [pendingDream, setPendingDream] = useState<DreamPendingPayload | null>(null);
  const [appliedDreamFlash, setAppliedDreamFlash] = useState<string | null>(null);
  const [restoreToast, setRestoreToast] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportFlash, setExportFlash] = useState<string | null>(null);
  /// Long-running mode: when on, submitting the input routes to
  /// `start_long_task` instead of `send_message`. The conversation panel
  /// shows a confirmation; progress is tracked in the sidebar's LongTaskPanel.
  const [longMode, setLongMode] = useState(false);
  const [longTaskFlash, setLongTaskFlash] = useState<string | null>(null);
  const [attachedFiles, setAttachedFiles] = useState<AttachedFile[]>([]);
  const [pastedImages, setPastedImages] = useState<PastedImage[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);

  async function startLongTask(text: string) {
    try {
      const taskId = await invoke<string>("start_long_task", { goal: text });
      setLongTaskFlash(
        `🚀 长跑任务已启动 (${taskId})。可关闭 app — 重启后能继续看到进度。`,
      );
      setTimeout(() => setLongTaskFlash(null), 6000);
      onLongTaskStarted?.();
    } catch (e) {
      setLongTaskFlash(`启动失败：${String(e)}`);
      setTimeout(() => setLongTaskFlash(null), 6000);
    }
  }

  async function handleExport() {
    setExporting(true);
    try {
      // 1. Ask backend to render the session as markdown.
      const exported = await invoke<{ filename: string; content: string }>(
        "export_session",
      );

      // 2. Open native save dialog with the suggested filename.
      const { save } = await import("@tauri-apps/plugin-dialog");
      const target = await save({
        defaultPath: exported.filename,
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!target) return; // user cancelled

      // 3. Write via the dedicated Rust command (dialog only returned path).
      await invoke("write_export", { path: target, content: exported.content });

      setExportFlash(`已导出到 ${target}`);
      setTimeout(() => setExportFlash(null), 4000);
    } catch (e) {
      const msg = String(e);
      console.error("[export]", msg);
      setExportFlash(`导出失败：${msg}`);
      setTimeout(() => setExportFlash(null), 6000);
    } finally {
      setExporting(false);
    }
  }

  // Show a transient "上次会话已恢复" banner when the backend hands us
  // history. Use a ref to gate: once shown for a given count, don't
  // re-trigger when the toast auto-dismisses. Without this guard the
  // effect re-runs every time `restoreToast` flips back to null and the
  // banner reappears in an infinite loop.
  const lastShownRestoreCountRef = useRef<number>(-1);
  useEffect(() => {
    if (restoredCount > 0 && lastShownRestoreCountRef.current !== restoredCount) {
      lastShownRestoreCountRef.current = restoredCount;
      setRestoreToast(`已恢复 ${restoredCount} 条上次会话消息`);
      const t = setTimeout(() => setRestoreToast(null), 5000);
      return () => clearTimeout(t);
    }
    return;
  }, [restoredCount]);

  // When the user switches sessions, reset the gate so the next session's
  // restore (potentially a different count) can show a banner again.
  useEffect(() => {
    lastShownRestoreCountRef.current = -1;
    setRestoreToast(null);
  }, [sessionEpoch]);

  // When the parent pushes a queued input (e.g. summarize prompt from the
  // agent panel), populate the textarea but do NOT auto-send — let the user
  // skim and adjust before submitting.
  useEffect(() => {
    if (!queuedInput) return;
    setInput(queuedInput.text);
  }, [queuedInput]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, thinking]);

  // Auto-dreaming events from backend (fired during clearSession)
  useEffect(() => {
    const offPending = listen<DreamPendingPayload>("dream-pending", (e) => {
      setPendingDream(e.payload);
    });
    const offApplied = listen<string>("dream-applied", (e) => {
      setAppliedDreamFlash(e.payload || "记忆已自动更新");
      setTimeout(() => setAppliedDreamFlash(null), 4000);
    });
    // Compaction completion banner — shows after manual or auto compaction.
    const offCompact = listen<{
      dropped: number;
      kept: number;
      auto?: boolean;
    }>("compaction-done", (e) => {
      const { dropped, kept, auto } = e.payload;
      const prefix = auto ? "🤖 自动压缩" : "🗜️ 已压缩";
      setAppliedDreamFlash(
        `${prefix}：折叠了 ${dropped} 条历史消息，保留 ${kept} 条最近上下文。`,
      );
      setTimeout(() => setAppliedDreamFlash(null), 5000);
    });
    // macOS native notifications for long task completion.
    // Permission is requested at mount, so we can fire-and-forget here.
    const offDone = listen<{ task_id: string; goal: string; elapsed_secs: number; iterations: number }>(
      "long-task-done",
      (e) => {
        const { goal, iterations } = e.payload;
        const title = goal.length > 50 ? goal.slice(0, 50) + "…" : goal;
        sendNotification({ title: "✅ 长跑任务完成", body: `${title}（${iterations} 轮）` });
      },
    );
    const offFailed = listen<{ task_id: string; goal: string; error: string }>(
      "long-task-failed",
      (e) => {
        const { goal, error } = e.payload;
        const title = goal.length > 40 ? goal.slice(0, 40) + "…" : goal;
        sendNotification({ title: "❌ 长跑任务失败", body: `${title}: ${error.slice(0, 80)}` });
      },
    );
    return () => {
      offPending.then((f) => f());
      offApplied.then((f) => f());
      offCompact.then((f) => f());
      offDone.then((f) => f());
      offFailed.then((f) => f());
    };
  }, []);

  async function handleCompact() {
    try {
      const report = await invoke<{ dropped: number; kept: number } | null>(
        "compact_session_now",
      );
      if (!report) {
        setAppliedDreamFlash(
          "ℹ️ 会话太短或当前位于工具调用中间，没有可安全压缩的部分。",
        );
        setTimeout(() => setAppliedDreamFlash(null), 4000);
      }
      // success path uses the listener above
    } catch (e) {
      setAppliedDreamFlash(`压缩失败：${String(e)}`);
      setTimeout(() => setAppliedDreamFlash(null), 6000);
    }
  }

  async function attachFilePaths(paths: string[]) {
    const results: AttachedFile[] = [];
    for (const path of paths) {
      try {
        const content = await invoke<string>("read_attachment", { path });
        const name = path.split("/").pop() ?? path;
        results.push({ name, content });
      } catch (e) {
        setAppliedDreamFlash(`无法读取文件：${String(e)}`);
        setTimeout(() => setAppliedDreamFlash(null), 4000);
      }
    }
    if (results.length > 0) {
      setAttachedFiles((prev) => [...prev, ...results]);
    }
  }

  async function handleAttachClick() {
    const selected = await openDialog({
      multiple: true,
      filters: [
        { name: "文本 / 代码", extensions: ["txt", "md", "ts", "tsx", "js", "jsx", "py", "rs", "go", "java", "c", "cpp", "h", "json", "yaml", "yml", "toml", "csv", "html", "css", "sh", "sql"] },
        { name: "所有文件", extensions: ["*"] },
      ],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    await attachFilePaths(paths);
  }

  // Wire up Tauri's native drag-drop + notification permission at mount.
  // React's onDrop gives File objects without real FS paths in the WebView;
  // getCurrentWebview().onDragDropEvent() is the only API that delivers them.
  useEffect(() => {
    // Request notification permission proactively so it's ready before any
    // long task finishes (avoids the permission dialog appearing mid-task).
    isPermissionGranted().then((granted) => {
      if (!granted) requestPermission().catch(() => {});
    }).catch(() => {});

    let unlisten: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((e) => {
      const ev = e.payload;
      if (ev.type === "enter" || ev.type === "over") {
        setDragOver(true);
      } else if (ev.type === "leave") {
        setDragOver(false);
      } else if (ev.type === "drop") {
        setDragOver(false);
        if (ev.paths && ev.paths.length > 0) {
          attachFilePaths(ev.paths);
        }
      }
    }).then((fn) => { unlisten = fn; }).catch(() => {});

    return () => { unlisten?.(); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function removeAttachment(name: string) {
    setAttachedFiles((prev) => prev.filter((f) => f.name !== name));
  }

  function removePastedImage(idx: number) {
    setPastedImages((prev) => prev.filter((_, i) => i !== idx));
  }

  function handlePaste(e: React.ClipboardEvent<HTMLTextAreaElement>) {
    const items = Array.from(e.clipboardData.items);
    const imageItems = items.filter((item) => item.type.startsWith("image/"));
    if (imageItems.length === 0) return; // let normal text paste proceed

    e.preventDefault();
    for (const item of imageItems) {
      const file = item.getAsFile();
      if (!file) continue;
      const reader = new FileReader();
      reader.onload = (ev) => {
        const dataUrl = ev.target?.result as string;
        if (!dataUrl) return;
        // data:image/png;base64,<data>  →  strip the prefix
        const commaIdx = dataUrl.indexOf(",");
        const raw = dataUrl.slice(commaIdx + 1);
        const media_type = item.type; // e.g. "image/png"
        setPastedImages((prev) => [...prev, { preview: dataUrl, data: raw, media_type }]);
      };
      reader.readAsDataURL(file);
    }
  }

  // Esc to cancel a running turn
  useEffect(() => {
    if (!thinking) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        cancelTurn();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [thinking, cancelTurn]);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const text = input.trim();
    if (!text && attachedFiles.length === 0 && pastedImages.length === 0) return;

    // Local slash-command interception (desktop-side only — never sent to API)
    if (text === "/clear") {
      setInput("");
      if (window.confirm("清空当前会话？\n\n⚠️ 历史消息将被永久删除，无法恢复。")) {
        clearSession();
      }
      return;
    }
    if (text === "/dream") {
      setInput("");
      setShowDream(true);
      return;
    }
    if (text === "/settings") {
      setInput("");
      setShowSettings(true);
      return;
    }
    if (text === "/agents") {
      setInput("");
      // Right panel is always visible — just flash a hint
      setAppliedDreamFlash("OPC Agents 在右侧 panel 显示");
      setTimeout(() => setAppliedDreamFlash(null), 2500);
      return;
    }
    if (text === "/memory") {
      setInput("");
      setShowDream(true);
      return;
    }

    // Prepend attached file contents as fenced code blocks
    let fullText = text;
    if (attachedFiles.length > 0) {
      const blocks = attachedFiles
        .map((f) => {
          const ext = f.name.split(".").pop() ?? "";
          return `【附件：${f.name}】\n\`\`\`${ext}\n${f.content}\n\`\`\``;
        })
        .join("\n\n");
      fullText = blocks + (text ? "\n\n" + text : "");
      setAttachedFiles([]);
    }

    setInput("");
    const images = pastedImages.map(({ data, media_type }) => ({ data, media_type }));
    if (pastedImages.length > 0) setPastedImages([]);

    if (longMode) {
      startLongTask(fullText);
    } else {
      sendMessage(fullText, images.length > 0 ? images : undefined);
    }
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit(e as unknown as React.FormEvent);
    }
  }

  return (
    <>
      <div className="flex-1 flex flex-col min-w-0">
        {/* Header */}
        <div className="h-12 flex items-center justify-between px-4 border-b border-[#333] bg-[#1e1e1e] flex-shrink-0">
          <div className="flex items-center gap-2">
            <span className="text-[#ff8c00] font-bold text-sm">OPC</span>
            <span className="text-[#888] text-xs">CEO Agent</span>
            <ContextHealthBadge />
          </div>
          <div className="flex items-center gap-3">
            <button
              onClick={handleExport}
              disabled={messages.length === 0 || exporting}
              className="text-xs text-[#666] hover:text-[#ff8c00] transition-colors flex items-center gap-1 disabled:opacity-30 disabled:cursor-not-allowed"
              title="导出当前对话为 Markdown 文件"
            >
              <span>{exporting ? "⏳" : "📥"}</span>{" "}
              {exporting ? "导出中…" : "导出"}
            </button>
            <button
              onClick={() => setLongMode((v) => !v)}
              className={`text-xs transition-colors flex items-center gap-1 ${
                longMode
                  ? "text-[#ff8c00] font-medium"
                  : "text-[#666] hover:text-[#ff8c00]"
              }`}
              title="长跑模式：提交后任务在后台跑（关 app 也行），可在左侧 panel 看进度"
            >
              <span>{longMode ? "🟠" : "🚀"}</span>
              {longMode ? "长跑 ON" : "长跑"}
            </button>
            <button
              onClick={() => setShowDream(true)}
              className="text-xs text-[#666] hover:text-[#ff8c00] transition-colors flex items-center gap-1"
              title="Dreaming — 让 agent 整合长期记忆"
            >
              <span>🌙</span> Dream
            </button>
            <button
              onClick={handleCompact}
              className="text-xs text-[#666] hover:text-[#ff8c00] transition-colors flex items-center gap-1"
              title="压缩当前会话历史 — 把旧消息浓缩成摘要，腾出上下文窗口"
            >
              <span>🗜️</span> 压缩
            </button>
            <button
              onClick={() => {
                if (window.confirm("清空当前会话？\n\n⚠️ 历史消息将被永久删除，无法恢复。\n（若想保留历史，请用「新建会话」代替）")) {
                  clearSession();
                }
              }}
              className="text-xs text-[#666] hover:text-red-400 transition-colors"
              title="永久删除本会话历史并开始新会话"
            >
              清空会话
            </button>
            <button
              onClick={() => setShowSettings(true)}
              className="text-xs text-[#666] hover:text-[#ff8c00] transition-colors flex items-center gap-1"
              title="Settings"
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
              设置
            </button>
          </div>
        </div>

        {/* Messages */}
        <div className="flex-1 overflow-y-auto px-4 py-4">
          {messages.length === 0 && !thinking ? (
            <div className="h-full flex flex-col items-center justify-center text-center px-8">
              <div className="text-4xl mb-4">🏢</div>
              <h1 className="text-lg font-semibold text-[#e5e5e5] mb-2">
                OPC CEO Agent
              </h1>
              <p className="text-sm text-[#888] max-w-sm">
                告诉我你想做什么，我来分解任务并调配专业团队完成它。
              </p>
            </div>
          ) : (
            <>
              {messages.map((msg) => (
                <MessageBubble
                  key={msg.id}
                  role={msg.role}
                  text={msg.text}
                  inputTokens={msg.inputTokens}
                  outputTokens={msg.outputTokens}
                  toolCalls={msg.toolCalls}
                  inProgress={msg.inProgress}
                  iteration={msg.iteration}
                />
              ))}
              {/* Show thinking dots only when there is no live (in-progress)
                  bubble yet — i.e. between sendMessage and the first
                  turn-start event from the backend. Once a streaming bubble
                  exists, it has its own "分析中…" marker. */}
              {thinking && !messages.some((m) => m.inProgress) && (
                <div className="flex justify-start mb-4">
                  <div className="bg-[#2d2d2d] rounded-2xl rounded-bl-sm px-4 py-3">
                    <ThinkingDots />
                  </div>
                </div>
              )}
              {error && (
                <div className="mb-4 px-4 py-3 bg-red-900/30 border border-red-800 rounded-lg text-sm text-red-300 flex items-start gap-2">
                  <span className="flex-1">{error}</span>
                  {error.toLowerCase().includes("api key") || error.toLowerCase().includes("api_key") ? (
                    <button
                      onClick={() => setShowSettings(true)}
                      className="flex-shrink-0 text-xs bg-red-800 hover:bg-red-700 px-2 py-1 rounded transition-colors"
                    >
                      打开设置
                    </button>
                  ) : null}
                </div>
              )}
            </>
          )}
          <div ref={bottomRef} />
        </div>

        {/* Input */}
        <div
          className={`flex-shrink-0 px-4 py-3 border-t border-[#333] bg-[#1e1e1e] transition-colors ${dragOver ? "bg-[#2a2010] border-[#ff8c00]/50" : ""}`}
        >
          {/* Pasted image thumbnails */}
          {pastedImages.length > 0 && (
            <div className="flex flex-wrap gap-2 mb-2">
              {pastedImages.map((img, idx) => (
                <div key={idx} className="relative group">
                  <img
                    src={img.preview}
                    alt={`图片 ${idx + 1}`}
                    className="h-16 w-auto max-w-[120px] object-cover rounded border border-[#ff8c00]/40"
                  />
                  <button
                    type="button"
                    onClick={() => removePastedImage(idx)}
                    className="absolute -top-1 -right-1 w-4 h-4 bg-red-600 rounded-full text-white text-[10px] leading-none flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}
          {/* Attached file chips */}
          {attachedFiles.length > 0 && (
            <div className="flex flex-wrap gap-1.5 mb-2">
              {attachedFiles.map((f) => (
                <span
                  key={f.name}
                  className="inline-flex items-center gap-1 px-2 py-0.5 bg-[#2d2d2d] border border-[#ff8c00]/40 rounded text-xs text-[#ff8c00]"
                >
                  <span>📎</span>
                  <span className="max-w-[180px] truncate">{f.name}</span>
                  <button
                    type="button"
                    onClick={() => removeAttachment(f.name)}
                    className="ml-0.5 text-[#888] hover:text-red-400 transition-colors"
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
          )}
          {dragOver && (
            <div className="mb-2 text-center text-xs text-[#ff8c00] animate-pulse">
              放开以附加文件
            </div>
          )}
          <form onSubmit={handleSubmit} className="flex gap-2 items-end">
            <PlusMenu
              onInsert={(text) => {
                setInput((prev) => (prev ? prev + "\n" + text : text));
              }}
            />
            {/* Attach file button */}
            <button
              type="button"
              onClick={handleAttachClick}
              disabled={thinking}
              title="附加文件（或直接拖文件到输入区）"
              className="flex-shrink-0 w-8 h-8 flex items-center justify-center rounded-lg text-[#666] hover:text-[#ff8c00] hover:bg-[#2d2d2d] transition-colors disabled:opacity-30"
            >
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
              </svg>
            </button>
            <textarea
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              onPaste={handlePaste}
              placeholder={
                thinking
                  ? "Agent 正在工作… 点击右侧 ■ 中止"
                  : dragOver
                    ? "放开以附加文件…"
                    : longMode
                      ? "🚀 长跑模式 — 提交后在后台跑（可关 app），从左侧 panel 看进度"
                      : "输入你的需求… (Enter 发送，Shift+Enter 换行；截图可直接 Ctrl+V 粘贴)"
              }
              rows={1}
              disabled={thinking}
              className="flex-1 resize-none bg-[#2d2d2d] text-[#e5e5e5] text-sm rounded-xl px-4 py-2.5 placeholder-[#555] border border-[#444] focus:outline-none focus:border-[#ff8c00] transition-colors disabled:opacity-50"
              style={{ minHeight: "40px", maxHeight: "160px" }}
              onInput={(e) => {
                const el = e.currentTarget;
                el.style.height = "auto";
                el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
              }}
            />
            {thinking ? (
              <button
                type="button"
                onClick={cancelTurn}
                title="中止当前对话 (Esc)"
                className="flex-shrink-0 w-10 h-10 rounded-xl bg-[#a33] text-white flex items-center justify-center hover:bg-[#c44] transition-colors animate-pulse"
              >
                {/* Square stop icon */}
                <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
                  <rect x="5" y="5" width="14" height="14" rx="1.5" />
                </svg>
              </button>
            ) : (
              <button
                type="submit"
                disabled={!input.trim() && attachedFiles.length === 0 && pastedImages.length === 0}
                className="flex-shrink-0 w-10 h-10 rounded-xl bg-[#ff8c00] text-white flex items-center justify-center disabled:opacity-40 hover:bg-[#e07800] transition-colors"
              >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <path d="M22 2L11 13" />
                  <path d="M22 2L15 22l-4-9-9-4 20-7z" />
                </svg>
              </button>
            )}
          </form>
        </div>
      </div>

      {showSettings && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          onSaved={async () => {
            // Only close the modal — settings are saved to disk and the
            // backend reconnects automatically. Do NOT clear the session;
            // the user's conversation history should survive a settings change.
            setShowSettings(false);
          }}
        />
      )}


      {showDream && <DreamReviewModal onClose={() => setShowDream(false)} />}

      {pendingDream && (
        <DreamReviewModal
          initialResult={pendingDream}
          onClose={() => setPendingDream(null)}
        />
      )}

      {appliedDreamFlash && (
        <div className="fixed top-4 right-4 z-50 px-4 py-2 bg-[#1e3a1e] border border-green-700 rounded-lg text-sm text-green-300 shadow-lg flex items-center gap-2">
          <span>🌙</span>
          <span>{appliedDreamFlash}</span>
        </div>
      )}

      {restoreToast && (
        <div className="fixed top-4 right-4 z-50 px-4 py-2 bg-[#2a2a3a] border border-[#444] rounded-lg text-sm text-[#ddd] shadow-lg flex items-center gap-2">
          <span>↩️</span>
          <span>{restoreToast}</span>
        </div>
      )}

      {exportFlash && (
        <div className="fixed top-4 right-4 z-50 px-4 py-2 bg-[#1e3a3a] border border-cyan-700 rounded-lg text-sm text-cyan-300 shadow-lg flex items-center gap-2 max-w-md">
          <span>📥</span>
          <span className="break-all">{exportFlash}</span>
        </div>
      )}

      {longTaskFlash && (
        <div className="fixed top-4 right-4 z-50 px-4 py-2 bg-[#3a2e1a] border border-[#ff8c00]/40 rounded-lg text-sm text-[#ff8c00] shadow-lg flex items-center gap-2 max-w-md">
          <span className="break-all">{longTaskFlash}</span>
        </div>
      )}
    </>
  );
}

function ThinkingDots() {
  return (
    <div className="flex gap-1 items-center h-4">
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          className="w-1.5 h-1.5 rounded-full bg-[#888] animate-bounce"
          style={{ animationDelay: `${i * 0.15}s` }}
        />
      ))}
    </div>
  );
}
