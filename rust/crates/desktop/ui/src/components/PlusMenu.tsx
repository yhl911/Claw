import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

interface SlashCommandInfo {
  name: string;
  summary: string;
}

interface Props {
  /// Called with content to insert into the chat input. Caller decides
  /// whether to replace or append.
  onInsert: (text: string) => void;
}

export function PlusMenu({ onInsert }: Props) {
  const [open, setOpen] = useState(false);
  const [showSlash, setShowSlash] = useState(false);
  const [slashCommands, setSlashCommands] = useState<SlashCommandInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function onClick(e: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
        setShowSlash(false);
      }
    }
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  async function handleAddFile() {
    setError(null);
    try {
      const selected = await openDialog({
        multiple: false,
        directory: false,
        title: "选择要附加的文件",
      });
      if (!selected || typeof selected !== "string") return;
      const content = await invoke<string>("read_attachment", { path: selected });
      const fileName = selected.split("/").pop() || selected;
      onInsert(`<file path="${selected}">\n${content}\n</file>\n\n`);
      setOpen(false);
      console.log(`[plus-menu] attached ${fileName} (${content.length} chars)`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleAddFolder() {
    setError(null);
    try {
      const selected = await openDialog({
        multiple: false,
        directory: true,
        title: "选择目录作为上下文",
      });
      if (!selected || typeof selected !== "string") return;
      onInsert(`<folder path="${selected}" />\n\n`);
      setOpen(false);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleShowSlash() {
    if (slashCommands.length === 0) {
      try {
        const cmds = await invoke<SlashCommandInfo[]>("list_slash_commands");
        setSlashCommands(cmds);
      } catch (e) {
        setError(String(e));
        return;
      }
    }
    setShowSlash(true);
  }

  function handlePickSlash(cmd: string) {
    onInsert(cmd + " ");
    setOpen(false);
    setShowSlash(false);
  }

  return (
    <div ref={containerRef} className="relative flex-shrink-0">
      <button
        type="button"
        onClick={() => {
          setOpen((v) => !v);
          setShowSlash(false);
        }}
        title="附加文件、目录或 slash 命令"
        className="w-9 h-9 rounded-lg bg-[#2d2d2d] border border-[#444] text-[#aaa] hover:bg-[#3a3a3a] hover:text-white transition-colors flex items-center justify-center text-lg leading-none"
      >
        +
      </button>

      {open && !showSlash && (
        <div className="absolute bottom-full left-0 mb-2 z-30 bg-[#2d2d2d] border border-[#444] rounded-lg shadow-xl py-1 min-w-[200px]">
          <MenuItem icon="📎" label="Add file" onClick={handleAddFile} />
          <MenuItem icon="📁" label="Add folder" onClick={handleAddFolder} />
          <MenuItem icon="/" label="Slash commands" onClick={handleShowSlash} />
          {error && (
            <div className="px-3 py-1.5 text-[10px] text-red-300 border-t border-[#444]">
              {error}
            </div>
          )}
        </div>
      )}

      {open && showSlash && (
        <div className="absolute bottom-full left-0 mb-2 z-30 bg-[#2d2d2d] border border-[#444] rounded-lg shadow-xl py-1 min-w-[280px] max-h-[300px] overflow-y-auto">
          <div className="px-3 py-1 text-[10px] uppercase text-[#888] tracking-wider border-b border-[#444]">
            Slash 命令
          </div>
          {slashCommands.map((cmd) => (
            <button
              key={cmd.name}
              onClick={() => handlePickSlash(cmd.name)}
              className="w-full px-3 py-1.5 text-left hover:bg-[#3a3a3a] transition-colors"
            >
              <div className="text-xs font-mono text-[#ddd]">{cmd.name}</div>
              <div className="text-[10px] text-[#888] mt-0.5">{cmd.summary}</div>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function MenuItem({
  icon,
  label,
  onClick,
}: {
  icon: string;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full px-3 py-1.5 text-left text-sm text-[#ddd] hover:bg-[#3a3a3a] transition-colors flex items-center gap-2"
    >
      <span className="w-5 text-center">{icon}</span>
      <span>{label}</span>
    </button>
  );
}
