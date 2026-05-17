import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface TaskSpec {
  task_id: string;
  goal: string;
  model: string;
  created_at: number;
  max_total_iterations: number | null;
  deadline: number | null;
}

interface TaskState {
  task_id: string;
  status: "pending" | "running" | "done" | "failed" | "cancelled" | "interrupted";
  current_iteration: number;
  input_tokens: number;
  output_tokens: number;
  started_at: number | null;
  last_heartbeat: number;
  completed_at: number | null;
  last_error: string | null;
  retry_count: number;
}

interface TaskInfo {
  spec: TaskSpec;
  state: TaskState;
}

const STATUS_STYLE: Record<TaskState["status"], { dot: string; label: string; cls: string }> = {
  pending: { dot: "bg-gray-500", label: "等待", cls: "text-gray-400" },
  running: { dot: "bg-yellow-500 animate-pulse", label: "运行中", cls: "text-yellow-400" },
  done: { dot: "bg-green-500", label: "完成", cls: "text-green-400" },
  failed: { dot: "bg-red-500", label: "失败", cls: "text-red-400" },
  cancelled: { dot: "bg-gray-500", label: "已取消", cls: "text-gray-400" },
  interrupted: { dot: "bg-orange-500", label: "中断", cls: "text-orange-400" },
};

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h${Math.floor((secs % 3600) / 60)}m`;
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

interface Props {
  /// Toggled by ChatPanel header — when this changes from false to true
  /// we refresh immediately so the new task shows up without waiting
  /// for the next poll tick.
  refreshTrigger?: number;
}

export function LongTaskPanel({ refreshTrigger }: Props) {
  const [tasks, setTasks] = useState<TaskInfo[]>([]);
  const [openTaskId, setOpenTaskId] = useState<string | null>(null);

  async function refresh() {
    try {
      const list = await invoke<TaskInfo[]>("list_long_tasks");
      setTasks(list);
    } catch (e) {
      console.warn("[LongTaskPanel] refresh failed:", e);
    }
  }

  useEffect(() => {
    refresh();
    const off = listen("long-task-changed", () => {
      refresh();
    });
    // Periodic poll for heartbeat updates (status.json mtime ticks every
    // 30s while a task runs; we want the iteration counter to feel live).
    const timer = setInterval(refresh, 5000);
    return () => {
      off.then((f) => f());
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    if (refreshTrigger !== undefined) refresh();
  }, [refreshTrigger]);

  const running = tasks.filter((t) => t.state.status === "running");
  const interrupted = tasks.filter((t) => t.state.status === "interrupted");
  const recent = tasks.slice(0, 20);

  if (tasks.length === 0) {
    return null;
  }

  return (
    <>
      <div className="border-t border-[#333] bg-[#181818]">
        <div className="px-3 py-2 flex items-center justify-between">
          <div className="flex items-center gap-2 text-xs text-[#aaa]">
            <span>🚀</span>
            <span className="font-semibold uppercase tracking-wider">长跑任务</span>
          </div>
          <div className="flex gap-2 text-[10px]">
            {running.length > 0 && (
              <span className="text-yellow-400">● {running.length}</span>
            )}
            {interrupted.length > 0 && (
              <span className="text-orange-400">⚠ {interrupted.length}</span>
            )}
            <span className="text-[#555]">{tasks.length}</span>
          </div>
        </div>
        <div className="max-h-[40vh] overflow-y-auto px-2 pb-2">
          {recent.map((task) => {
            const style = STATUS_STYLE[task.state.status];
            const goal = task.spec.goal;
            const goalShort = goal.length > 50 ? `${goal.slice(0, 50)}…` : goal;
            const startedAt = task.state.started_at;
            const completedAt = task.state.completed_at;
            const duration =
              startedAt && completedAt
                ? formatDuration(completedAt - startedAt)
                : startedAt
                  ? formatDuration(Math.floor(Date.now() / 1000) - startedAt)
                  : "";

            return (
              <button
                key={task.spec.task_id}
                onClick={() => setOpenTaskId(task.spec.task_id)}
                className="w-full text-left mb-1 p-2 rounded bg-[#242424] hover:bg-[#2a2a2a] border border-[#333] hover:border-[#ff8c00] transition-colors"
              >
                <div className="flex items-center gap-2 mb-1">
                  <span
                    className={`w-2 h-2 rounded-full flex-shrink-0 ${style.dot}`}
                  />
                  <span className={`text-[10px] font-medium ${style.cls}`}>
                    {style.label}
                  </span>
                  <span className="text-[10px] text-[#555] ml-auto">
                    iter {task.state.current_iteration}
                    {duration && ` · ${duration}`}
                  </span>
                </div>
                <p className="text-xs text-[#ddd] leading-snug line-clamp-2">
                  {goalShort}
                </p>
                <div className="text-[10px] text-[#666] mt-1 flex justify-between">
                  <span>{relativeTime(task.spec.created_at)}</span>
                  {task.state.input_tokens + task.state.output_tokens > 0 && (
                    <span>
                      {(
                        (task.state.input_tokens + task.state.output_tokens) /
                        1000
                      ).toFixed(1)}
                      K tok
                    </span>
                  )}
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {openTaskId && (
        <LongTaskDetailModal
          taskId={openTaskId}
          onClose={() => {
            setOpenTaskId(null);
            refresh();
          }}
        />
      )}
    </>
  );
}

function LongTaskDetailModal({
  taskId,
  onClose,
}: {
  taskId: string;
  onClose: () => void;
}) {
  const [info, setInfo] = useState<TaskInfo | null>(null);
  const [output, setOutput] = useState<string>("");
  const [busy, setBusy] = useState(false);

  async function refresh() {
    try {
      const i = await invoke<TaskInfo>("get_long_task", { taskId });
      setInfo(i);
      if (i.state.status === "done") {
        const o = await invoke<string>("read_long_task_output", { taskId });
        setOutput(o);
      }
    } catch (e) {
      console.warn("[LongTaskDetailModal] refresh failed:", e);
    }
  }

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 3000);
    return () => clearInterval(timer);
  }, [taskId]);

  async function handleCancel() {
    setBusy(true);
    try {
      await invoke("cancel_long_task", { taskId });
      await refresh();
    } catch (e) {
      console.error("[long-task] cancel failed:", e);
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!confirm("永久删除这个任务及其所有产物？")) return;
    setBusy(true);
    try {
      await invoke("delete_long_task", { taskId });
      onClose();
    } catch (e) {
      alert(`删除失败: ${e}`);
      setBusy(false);
    }
  }

  async function handleResume() {
    setBusy(true);
    try {
      await invoke("resume_long_task", { taskId });
      await refresh();
    } catch (e) {
      alert(`续跑失败: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  if (!info) {
    return (
      <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
        <div className="bg-[#242424] rounded-2xl px-6 py-4 text-sm text-[#aaa]">
          加载中…
        </div>
      </div>
    );
  }

  const style = STATUS_STYLE[info.state.status];
  const startedAt = info.state.started_at;
  const completedAt = info.state.completed_at;
  const isRunning = info.state.status === "running";
  const isTerminal = !["pending", "running"].includes(info.state.status);

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[720px] max-w-[95vw] h-[80vh] flex flex-col shadow-2xl border border-[#333]"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333]">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-base">🚀</span>
              <span className={`px-2 py-0.5 text-xs rounded ${style.cls} bg-[#1a1a1a]`}>
                {style.label}
              </span>
              <span className="text-xs text-[#666] font-mono">{taskId}</span>
            </div>
            <p className="text-sm text-[#e5e5e5] mt-1 line-clamp-2">
              {info.spec.goal}
            </p>
          </div>
          <button
            onClick={onClose}
            className="text-[#666] hover:text-[#aaa] text-xl leading-none ml-4"
          >
            ×
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-6 py-4 space-y-4">
          <div className="grid grid-cols-2 gap-3 text-xs">
            <Stat label="模型" value={info.spec.model} />
            <Stat label="迭代" value={String(info.state.current_iteration)} />
            <Stat
              label="开始"
              value={startedAt ? new Date(startedAt * 1000).toLocaleString() : "—"}
            />
            <Stat
              label="结束"
              value={completedAt ? new Date(completedAt * 1000).toLocaleString() : "—"}
            />
            <Stat
              label="耗时"
              value={
                startedAt && completedAt
                  ? formatDuration(completedAt - startedAt)
                  : startedAt
                    ? `${formatDuration(Math.floor(Date.now() / 1000) - startedAt)} (运行中)`
                    : "—"
              }
            />
            <Stat
              label="Token"
              value={`${(info.state.input_tokens / 1000).toFixed(1)}K↓ / ${(info.state.output_tokens / 1000).toFixed(1)}K↑`}
            />
          </div>

          {info.state.last_error && (
            <section>
              <h3 className="text-xs font-medium text-red-400 mb-1.5 uppercase tracking-wider">
                错误
              </h3>
              <pre className="text-xs text-red-300 bg-red-900/20 rounded-lg p-3 whitespace-pre-wrap break-words">
                {info.state.last_error}
              </pre>
            </section>
          )}

          {output && (
            <section>
              <h3 className="text-xs font-medium text-[#888] mb-1.5 uppercase tracking-wider">
                最终输出
              </h3>
              <pre className="text-xs text-[#ddd] bg-[#1a1a1a] rounded-lg p-3 overflow-x-auto whitespace-pre-wrap leading-relaxed font-mono">
                {output}
              </pre>
            </section>
          )}

          {isRunning && !output && (
            <section className="text-center py-8">
              <div className="text-3xl mb-2 animate-pulse">⏳</div>
              <p className="text-sm text-[#aaa]">任务运行中…</p>
              <p className="text-xs text-[#666] mt-1">
                心跳每 30 秒更新一次。可以关掉 app，下次启动可以继续看到进度。
              </p>
            </section>
          )}
        </div>

        <div className="flex justify-between items-center px-6 py-3 border-t border-[#333]">
          <span className="text-[10px] text-[#555]">每 3 秒自动刷新</span>
          <div className="flex gap-3">
            {isRunning && (
              <button
                onClick={handleCancel}
                disabled={busy}
                className="px-4 py-1.5 text-xs text-[#aaa] border border-[#444] hover:border-red-500 hover:text-red-400 rounded transition-colors disabled:opacity-30"
              >
                {busy ? "处理中…" : "取消任务"}
              </button>
            )}
            {/* Resume affordance for non-Running terminal states except
                the truly-Done one. The runner sends a continuation prompt
                instead of replaying the original goal. */}
            {["interrupted", "failed", "cancelled"].includes(
              info.state.status,
            ) && (
              <button
                onClick={handleResume}
                disabled={busy}
                className="px-4 py-1.5 text-xs bg-[#ff8c00]/10 hover:bg-[#ff8c00]/20 border border-[#ff8c00]/30 hover:border-[#ff8c00] rounded text-[#ff8c00] transition-colors disabled:opacity-30"
                title="加载已有上下文，从中断处继续"
              >
                {busy ? "启动中…" : "▶ 续跑"}
              </button>
            )}
            {isTerminal && (
              <button
                onClick={handleDelete}
                disabled={busy}
                className="px-4 py-1.5 text-xs text-[#666] hover:text-red-400 transition-colors disabled:opacity-30"
              >
                删除
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="text-[10px] text-[#666] uppercase tracking-wider">{label}</p>
      <p className="text-xs text-[#ddd] mt-0.5 break-all">{value}</p>
    </div>
  );
}
