import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

interface SkillInfo {
  name: string;
  description: string;
  source: string;
  enabled: boolean;
  path: string;
  imported_at: number;
}

interface RemoteSkill {
  name: string;
  path: string;
  kind: string;
}

interface Props {
  onClose: () => void;
}

type Tab = "local" | "remote" | "new";

export function SkillsPanel({ onClose }: Props) {
  const [tab, setTab] = useState<Tab>("local");
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Remote tab state
  const [repo, setRepo] = useState<string>("anthropics/skills");
  const [remote, setRemote] = useState<RemoteSkill[]>([]);
  const [remoteLoaded, setRemoteLoaded] = useState(false);
  const [importing, setImporting] = useState<string | null>(null);

  // New tab state
  const [newName, setNewName] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [newBody, setNewBody] = useState("");

  // Detail view
  const [detail, setDetail] = useState<{ name: string; content: string } | null>(null);

  async function refresh() {
    try {
      const list = await invoke<SkillInfo[]>("list_skills");
      setSkills(list);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function loadRemote() {
    setLoading(true);
    setError(null);
    try {
      const list = await invoke<RemoteSkill[]>("list_remote_skills", {
        repo: repo.trim() || null,
      });
      setRemote(list);
      setRemoteLoaded(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  async function handleImport(item: RemoteSkill) {
    setImporting(item.path);
    setError(null);
    try {
      await invoke("import_remote_skill", {
        repo: repo.trim() || null,
        path: item.path,
      });
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setImporting(null);
    }
  }

  async function handleDelete(name: string) {
    if (!confirm(`删除 skill '${name}' ？此操作不可恢复。`)) return;
    try {
      await invoke("delete_skill", { name });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleToggle(s: SkillInfo) {
    try {
      await invoke("toggle_skill", { name: s.name, enabled: !s.enabled });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleImportLocal() {
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "选择一个包含 SKILL.md 的目录",
      });
      if (!selected || typeof selected !== "string") return;
      await invoke("import_local_skill", { path: selected });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleViewDetail(name: string) {
    try {
      const content = await invoke<string>("read_skill", { name });
      setDetail({ name, content });
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleCreate() {
    setError(null);
    if (!newName.trim()) {
      setError("Skill 名称必填（kebab-case，不含空格和 '/'）");
      return;
    }
    if (!newBody.trim()) {
      setError("Skill 内容必填");
      return;
    }
    try {
      await invoke("create_skill", {
        name: newName.trim(),
        description: newDesc.trim(),
        body: newBody,
      });
      setNewName("");
      setNewDesc("");
      setNewBody("");
      setTab("local");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-[#1a1a1a] border border-[#333] rounded-xl w-full max-w-3xl max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-[#333]">
          <div className="flex items-center gap-2">
            <span className="text-lg">🧰</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">Skills 管理</h2>
          </div>
          <button
            onClick={onClose}
            className="text-[#888] hover:text-[#e5e5e5] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        {/* Tabs */}
        <div className="flex border-b border-[#333] px-5">
          {[
            { id: "local", label: "本地 Skills" },
            { id: "remote", label: "从公开仓库导入" },
            { id: "new", label: "新建" },
          ].map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id as Tab)}
              className={`px-3 py-2 text-sm transition-colors border-b-2 ${
                tab === t.id
                  ? "text-[#ff8c00] border-[#ff8c00]"
                  : "text-[#888] border-transparent hover:text-[#e5e5e5]"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>

        {error && (
          <div className="mx-5 mt-3 px-3 py-2 bg-red-900/30 border border-red-800 rounded text-xs text-red-300">
            {error}
          </div>
        )}

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-5 py-4">
          {tab === "local" && (
            <div className="space-y-2">
              <div className="flex justify-end">
                <button
                  onClick={handleImportLocal}
                  className="text-xs px-2 py-1 bg-[#2d2d2d] hover:bg-[#3a3a3a] text-[#aaa] hover:text-[#ff8c00] rounded border border-[#444] transition-colors"
                  title="选择本地一个包含 SKILL.md 的文件夹复制进来"
                >
                  📁 从本地文件夹导入
                </button>
              </div>
              {skills.length === 0 ? (
                <div className="text-center text-sm text-[#666] py-8">
                  还没有 skill。可以「新建」自己写、「从本地文件夹导入」、或「从公开仓库导入」。
                </div>
              ) : (
                skills.map((s) => (
                  <div
                    key={s.name}
                    className="bg-[#222] border border-[#333] rounded p-3 flex items-start gap-3"
                  >
                    <button
                      onClick={() => handleToggle(s)}
                      className={`flex-shrink-0 mt-1 w-10 h-5 rounded-full relative transition-colors ${
                        s.enabled ? "bg-[#ff8c00]" : "bg-[#444]"
                      }`}
                      title={s.enabled ? "已启用 — 点击禁用" : "已禁用 — 点击启用"}
                    >
                      <span
                        className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full transition-transform ${
                          s.enabled ? "translate-x-5" : "translate-x-0"
                        }`}
                      />
                    </button>
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-sm text-[#e5e5e5]">{s.name}</span>
                        <span
                          className={`text-[10px] px-1.5 py-0.5 rounded ${
                            s.source === "anthropic"
                              ? "bg-purple-900/40 text-purple-300"
                              : "bg-blue-900/40 text-blue-300"
                          }`}
                        >
                          {s.source}
                        </span>
                      </div>
                      <p className="text-xs text-[#888] mt-1 line-clamp-2">
                        {s.description || <span className="italic">无描述</span>}
                      </p>
                    </div>
                    <div className="flex flex-col gap-1">
                      <button
                        onClick={() => handleViewDetail(s.name)}
                        className="text-xs text-[#aaa] hover:text-[#ff8c00] transition-colors"
                      >
                        查看
                      </button>
                      <button
                        onClick={() => handleDelete(s.name)}
                        className="text-xs text-[#aaa] hover:text-red-400 transition-colors"
                      >
                        删除
                      </button>
                    </div>
                  </div>
                ))
              )}
            </div>
          )}

          {tab === "remote" && (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={repo}
                  onChange={(e) => setRepo(e.target.value)}
                  placeholder="owner/repo (默认 anthropics/skills)"
                  className="flex-1 px-3 py-1.5 bg-[#222] border border-[#333] rounded text-sm text-[#e5e5e5] focus:outline-none focus:border-[#ff8c00]"
                />
                <button
                  onClick={loadRemote}
                  disabled={loading}
                  className="px-3 py-1.5 bg-[#ff8c00] hover:bg-[#ff7700] disabled:opacity-50 text-black text-sm rounded transition-colors"
                >
                  {loading ? "下载中..." : "拉取列表"}
                </button>
              </div>
              <p className="text-xs text-[#666]">
                从公开 GitHub 仓库列出可导入的 skill。下载会拉取整个仓库 tarball
                （anthropics/skills 约 3.4 MB），第一次较慢，之后 1 小时内复用本地缓存。
                进度日志在终端可见。
              </p>
              {remoteLoaded && remote.length === 0 && (
                <div className="text-center text-sm text-[#666] py-6">
                  此仓库根目录没有可识别的 skill。请确认仓库结构正确（顶层目录应包含 SKILL.md）。
                </div>
              )}
              {remote.map((item) => {
                const already = skills.some((s) => s.name === item.name);
                return (
                  <div
                    key={item.path}
                    className="bg-[#222] border border-[#333] rounded p-3 flex items-center justify-between"
                  >
                    <div>
                      <div className="text-sm text-[#e5e5e5]">{item.name}</div>
                      <div className="text-xs text-[#666]">{item.path}</div>
                    </div>
                    <button
                      onClick={() => handleImport(item)}
                      disabled={already || importing !== null}
                      className="text-xs px-3 py-1 bg-[#333] hover:bg-[#444] disabled:opacity-40 disabled:cursor-not-allowed text-[#e5e5e5] rounded transition-colors"
                    >
                      {already
                        ? "已导入"
                        : importing === item.path
                          ? "导入中..."
                          : "导入"}
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          {tab === "new" && (
            <div className="space-y-3">
              <div>
                <label className="text-xs text-[#888] block mb-1">Skill 名称（kebab-case）</label>
                <input
                  type="text"
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                  placeholder="例如：weekly-report"
                  className="w-full px-3 py-1.5 bg-[#222] border border-[#333] rounded text-sm text-[#e5e5e5] focus:outline-none focus:border-[#ff8c00]"
                />
              </div>
              <div>
                <label className="text-xs text-[#888] block mb-1">描述（一行说明何时使用）</label>
                <input
                  type="text"
                  value={newDesc}
                  onChange={(e) => setNewDesc(e.target.value)}
                  placeholder="生成每周周报，含本周完成 / 下周计划 / 风险"
                  className="w-full px-3 py-1.5 bg-[#222] border border-[#333] rounded text-sm text-[#e5e5e5] focus:outline-none focus:border-[#ff8c00]"
                />
              </div>
              <div>
                <label className="text-xs text-[#888] block mb-1">SKILL.md 主体（agent 加载后会读完整内容）</label>
                <textarea
                  value={newBody}
                  onChange={(e) => setNewBody(e.target.value)}
                  rows={12}
                  placeholder={"## 使用场景\n...\n\n## 步骤\n1. ...\n2. ..."}
                  className="w-full px-3 py-2 bg-[#222] border border-[#333] rounded text-sm text-[#e5e5e5] focus:outline-none focus:border-[#ff8c00] font-mono"
                />
              </div>
              <button
                onClick={handleCreate}
                className="px-4 py-2 bg-[#ff8c00] hover:bg-[#ff7700] text-black text-sm rounded transition-colors"
              >
                创建 Skill
              </button>
            </div>
          )}
        </div>

        {detail && (
          <div
            className="fixed inset-0 bg-black/70 z-[60] flex items-center justify-center p-4"
            onClick={() => setDetail(null)}
          >
            <div
              className="bg-[#1a1a1a] border border-[#333] rounded-xl w-full max-w-2xl max-h-[80vh] flex flex-col"
              onClick={(e) => e.stopPropagation()}
            >
              <div className="flex items-center justify-between px-5 py-3 border-b border-[#333]">
                <span className="text-sm font-medium text-[#e5e5e5]">{detail.name}</span>
                <button
                  onClick={() => setDetail(null)}
                  className="text-[#888] hover:text-[#e5e5e5] text-xl leading-none"
                >
                  ×
                </button>
              </div>
              <pre className="flex-1 overflow-auto p-4 text-xs text-[#ccc] whitespace-pre-wrap font-mono">
                {detail.content}
              </pre>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
