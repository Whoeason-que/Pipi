import { useRef, useState } from "react";
import { useDialogFocus } from "../components/use-dialog-focus";
import { IconClose } from "../icons";
import { normalizeWorkspacePath } from "../app/workspaces";

interface CreateWorkspaceModalProps {
  onCreated: (path: string, name?: string) => void;
  onCancel: () => void;
}

export function CreateWorkspaceModal({
  onCreated,
  onCancel,
}: CreateWorkspaceModalProps) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useDialogFocus(root, onCancel);

  const handleSubmit = (e?: React.FormEvent) => {
    e?.preventDefault();
    const normalized = normalizeWorkspacePath(path);
    if (!normalized) {
      setError("请输入有效的工作区目录路径");
      return;
    }
    setError(null);
    onCreated(normalized, name.trim() || undefined);
  };

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        ref={root}
        className="modal"
        style={{ maxWidth: 480 }}
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-workspace-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="create-workspace-title">新建工作区</h2>
          <button
            type="button"
            className="icon-btn close-btn"
            onClick={onCancel}
            title="关闭"
          >
            <IconClose />
          </button>
        </div>

        <form onSubmit={handleSubmit}>
          <div className="modal-section" style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            {error && (
              <div className="error" role="alert" style={{ margin: 0 }}>
                <span>{error}</span>
              </div>
            )}

            <div className="field">
              <label className="label" htmlFor="workspace-path-input">
                工作区目录路径 <span style={{ color: "var(--danger)" }}>*</span>
              </label>
              <input
                id="workspace-path-input"
                className="input mono"
                placeholder="例如：/home/user/projects/my-app"
                value={path}
                onChange={(e) => {
                  setPath(e.target.value);
                  if (error) setError(null);
                }}
                autoFocus
              />
              <div className="hint">
                本地文件系统上的工作目录绝对路径（如现有代码仓库或新目录）。
              </div>
            </div>

            <div className="field">
              <label className="label" htmlFor="workspace-name-input">
                显示名称（可选）
              </label>
              <input
                id="workspace-name-input"
                className="input"
                placeholder="留空则自动提取目录末尾的文件夹名"
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </div>

            <div className="hint" style={{ lineHeight: 1.6 }}>
              选择该工作区后，侧栏与主区将仅展示归属于该工作区的 Agent 与内容；新建 Agent 时也将默认填充该目录。
            </div>
          </div>

          <div className="modal-actions">
            <button type="button" className="btn ghost" onClick={onCancel}>
              取消
            </button>
            <button
              type="submit"
              className="btn primary"
              disabled={!path.trim()}
            >
              创建并切换
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
