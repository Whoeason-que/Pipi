import { useCallback, useEffect, useState } from "react";
import {
  formatRuntimeError
} from "../chat-runtime";
import { useI18n } from "../i18n";
import {
  invoke
} from "../platform";
import type { AgentEditableFiles } from "../types";

const EMPTY_FILES: AgentEditableFiles = { instructions: [], memory: [], memoryPrefix: "" };


// ============ Agent 文件编辑器（AGENTS.md / memory/*.md） ============

interface AgentFileEditorProps {
  agentName: string;
  onError: (msg: string) => void;
  disabled?: boolean;
  onSaved?: () => void | Promise<void>;
}

export default function AgentFileEditor({ agentName, onError, disabled = false, onSaved }: AgentFileEditorProps) {
  const { t } = useI18n();
  const [files, setFiles] = useState<AgentEditableFiles>(EMPTY_FILES);
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
      return EMPTY_FILES;
    }
  }, [agentName, onError]);

  useEffect(() => {
    void (async () => {
      const list = await refreshFiles();
      // 默认打开契约声明的第一个指令文件（位置由 agent.toml 决定）
      const firstInstruction = list.instructions[0];
      if (firstInstruction) void openFile(firstInstruction);
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
    const relPath = `${files.memoryPrefix}/${name}.md`;
    if (files.memory.includes(relPath)) {
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
      <span className="k">{t("agents.instructionsAndMemory")}</span>
      <div className="v">
        <div className="file-tabs">
          {[...files.instructions, ...files.memory].map((file) => (
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
            placeholder={t("agents.newMemoryFilePlaceholder")}
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
            {t("agents.newFile")}
          </button>
        </div>
        <textarea
          className="mono file-editor"
          value={loading ? t("common.loading") : content}
          onChange={(e) => setContent(e.target.value)}
          disabled={disabled || !activeFile || loading}
          placeholder={
            activeFile ? undefined : t("agents.selectFilePlaceholder")
          }
          rows={8}
          spellCheck={false}
        />
        <div className="hint">
          {activeFile
            ? t("agents.fileHint", { file: activeFile })
            : t("agents.fileHintEmpty")}
        </div>
        <button
          type="button"
          className="btn primary"
          disabled={disabled || !dirty || saving}
          onClick={() => void saveFile()}
        >
          {saving
            ? t("agents.saving")
            : disabled
              ? t("agents.testRunning")
              : dirty
                ? t("agents.saveFile")
                : t("agents.saved")}
        </button>
      </div>
    </div>
  );
}
