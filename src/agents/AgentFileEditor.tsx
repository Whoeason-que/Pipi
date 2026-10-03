import { useCallback, useEffect, useState } from "react";
import {
  formatRuntimeError
} from "../chat-runtime";
import {
  invoke
} from "../platform";


// ============ Agent 文件编辑器（AGENTS.md / memory/*.md） ============

interface AgentFileEditorProps {
  agentName: string;
  onError: (msg: string) => void;
  disabled?: boolean;
  onSaved?: () => void | Promise<void>;
}

export default function AgentFileEditor({ agentName, onError, disabled = false, onSaved }: AgentFileEditorProps) {
  const [files, setFiles] = useState<string[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [savedContent, setSavedContent] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [newFileName, setNewFileName] = useState("");

  const openFile = useCallback(async (relPath: string) => {
    setActiveFile(relPath);
    setLoading(true);
    try {
      const text = await invoke("read_agent_file", {
        agentName,
        relPath,
      });
      setContent(text);
      setSavedContent(text);
    } catch (errorValue) {
      setActiveFile(null);
      onError(formatRuntimeError(errorValue));
    } finally {
      setLoading(false);
    }
  }, [agentName, onError]);

  const refreshFiles = useCallback(async () => {
    try {
      const list = await invoke("list_agent_files", { agentName });
      setFiles(list);
      return list;
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
      return [];
    }
  }, [agentName, onError]);

  useEffect(() => {
    void (async () => {
      const list = await refreshFiles();
      // 默认打开 AGENTS.md（存在时）
      if (list.includes("AGENTS.md")) void openFile("AGENTS.md");
    })();
    // agentName 变化时重置
    setActiveFile(null);
    setContent("");
    setSavedContent("");
    setNewFileName("");
  }, [refreshFiles, openFile, agentName]);

  const saveFile = async () => {
    if (!activeFile || saving || disabled || content === savedContent) return;
    setSaving(true);
    try {
      await invoke("write_agent_file", {
        agentName,
        relPath: activeFile,
        content,
      });
      setSavedContent(content);
      await onSaved?.();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSaving(false);
    }
  };

  const createMemoryFile = async () => {
    if (disabled) return;
    const name = newFileName.trim().replace(/\.md$/, "");
    if (!name || name.includes("/")) return;
    const relPath = `memory/${name}.md`;
    if (files.includes(relPath)) {
      void openFile(relPath);
      setNewFileName("");
      return;
    }
    try {
      await invoke("write_agent_file", { agentName, relPath, content: "" });
      setNewFileName("");
      await refreshFiles();
      await openFile(relPath);
      await onSaved?.();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    }
  };

  const dirty = activeFile !== null && content !== savedContent;

  return (
    <div className="drow file-editor-row">
      <span className="k">AGENTS.md · memory/</span>
      <div className="v">
        <div className="file-tabs">
          {files.map((file) => (
            <button
              key={file}
              type="button"
              className={`file-tab mono${file === activeFile ? " active" : ""}`}
              disabled={disabled}
              onClick={() => void openFile(file)}
            >
              {file}
            </button>
          ))}
        </div>
        <div className="file-new">
          <input
            className="mono"
            value={newFileName}
            disabled={disabled}
            onChange={(e) => setNewFileName(e.target.value)}
            placeholder="新建 memory 文件，例如 user-prefs"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void createMemoryFile();
              }
            }}
          />
          <button
            type="button"
            className="btn ghost"
            disabled={disabled || !newFileName.trim() || newFileName.trim().includes("/")}
            onClick={() => void createMemoryFile()}
          >
            新建
          </button>
        </div>
        <textarea
          className="mono file-editor"
          value={loading ? "加载中…" : content}
          onChange={(e) => setContent(e.target.value)}
          disabled={disabled || !activeFile || loading}
          placeholder={activeFile ? undefined : "选择或新建一个文件开始编辑"}
          rows={8}
          spellCheck={false}
        />
        <div className="hint">
          {activeFile ? (
            <>
              {activeFile} —— AGENTS.md 每次对话开始时注入为系统指令；memory
              索引常驻上下文、正文由模型按需读取。改动需手动保存。
            </>
          ) : (
            "AGENTS.md 是系统级指令，memory/ 是 Agent 的持久记忆"
          )}
        </div>
        <button
          type="button"
          className="btn primary"
          disabled={disabled || !dirty || saving}
          onClick={() => void saveFile()}
        >
          {saving ? "保存中…" : disabled ? "测试运行中" : dirty ? "保存文件" : "已保存"}
        </button>
      </div>
    </div>
  );
}
