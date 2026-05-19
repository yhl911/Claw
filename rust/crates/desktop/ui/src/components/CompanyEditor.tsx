import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  onClose: () => void;
}

export function CompanyEditor({ onClose }: Props) {
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  useEffect(() => {
    invoke<string>("get_company_context").then(setText).catch(() => {});
  }, []);

  async function handleSave() {
    setSaving(true);
    try {
      await invoke("save_company_context", { text });
      setToast("已保存");
      setTimeout(() => setToast(null), 2000);
    } catch (e) {
      setToast(`保存失败：${String(e)}`);
      setTimeout(() => setToast(null), 4000);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className="bg-[#242424] rounded-2xl w-[560px] h-[600px] shadow-2xl border border-[#333] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#333] flex-shrink-0">
          <div className="flex items-center gap-2">
            <span className="text-lg">🏢</span>
            <h2 className="text-base font-semibold text-[#e5e5e5]">公司档案</h2>
          </div>
          <button
            onClick={onClose}
            className="text-[#666] hover:text-[#aaa] transition-colors text-xl leading-none"
          >
            ×
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 flex flex-col px-6 py-4 min-h-0">
          <p className="text-xs text-[#666] mb-3">
            CEO 会在每次对话中记住这些信息。支持 Markdown 格式。
          </p>
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            className="flex-1 bg-[#1a1a1a] text-[#e5e5e5] text-sm rounded-lg px-3 py-2.5 border border-[#444] focus:outline-none focus:border-[#ff8c00] resize-none font-mono placeholder-[#444] transition-colors"
            placeholder={`描述你的公司，CEO 会在每次对话中记住这些信息。

示例：
## 公司简介
[公司名] 是一家做 [产品] 的初创公司，目标用户是 [人群]。

## 产品
- 核心功能：...
- 技术栈：...

## 当前阶段
[种子/A轮]，[X] 名全职，月收入 [Y]。

## 重要约束
- ...`}
          />
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between px-6 py-4 border-t border-[#333] flex-shrink-0">
          <div className="h-6">
            {toast && (
              <span
                className={`text-xs px-2 py-1 rounded ${
                  toast === "已保存"
                    ? "text-green-400 bg-green-900/30"
                    : "text-red-400 bg-red-900/30"
                }`}
              >
                {toast}
              </span>
            )}
          </div>
          <div className="flex gap-3">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm text-[#888] hover:text-[#ccc] transition-colors"
            >
              取消
            </button>
            <button
              onClick={handleSave}
              disabled={saving}
              className="px-5 py-2 text-sm bg-[#ff8c00] text-white rounded-lg hover:bg-[#e07800] disabled:opacity-50 transition-colors font-medium"
            >
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
