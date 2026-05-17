import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface MemoryFile {
  name: string;
  content: string;
}

interface Props {
  onClose: () => void;
}

export function MemoryPanel({ onClose }: Props) {
  const [files, setFiles] = useState<MemoryFile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);

  useEffect(() => {
    invoke<MemoryFile[]>("list_memory_files")
      .then((f) => {
        setFiles(f);
        if (f.length > 0) setSelected(f[0].name);
      })
      .catch((e) => setError(String(e)));
  }, []);

  const current = files.find((f) => f.name === selected) ?? null;

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="bg-[#1a1a1a] border border-[#333] rounded-xl w-full max-w-4xl max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-[#333]">
          <div className="flex items-center gap-2">
            <span className="text-lg">🧠</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">长期记忆</h2>
            <span className="text-xs text-[#666]">— .claw/memory/ 下由 dreaming 固化的内容</span>
          </div>
          <button
            onClick={onClose}
            className="text-[#888] hover:text-[#e5e5e5] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        <div className="flex-1 overflow-hidden flex">
          <div className="w-48 border-r border-[#333] overflow-y-auto py-2">
            {files.length === 0 ? (
              <p className="text-xs text-[#555] px-3 py-4">还没有记忆文件</p>
            ) : (
              files.map((f) => (
                <button
                  key={f.name}
                  onClick={() => setSelected(f.name)}
                  className={`w-full text-left px-3 py-1.5 text-xs transition-colors ${
                    selected === f.name
                      ? "bg-[#2d2d2d] text-[#ff8c00]"
                      : "text-[#aaa] hover:bg-[#222]"
                  }`}
                >
                  {f.name}
                </button>
              ))
            )}
          </div>
          <div className="flex-1 overflow-auto p-4">
            {error && (
              <div className="mb-3 px-3 py-2 bg-red-900/30 border border-red-800 rounded text-xs text-red-300">
                {error}
              </div>
            )}
            {current ? (
              <pre className="text-xs text-[#ccc] whitespace-pre-wrap font-mono">
                {current.content || <span className="italic text-[#555]">（空文件）</span>}
              </pre>
            ) : (
              <p className="text-xs text-[#555]">选择左侧文件查看内容</p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
