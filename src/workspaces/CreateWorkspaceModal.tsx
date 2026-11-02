import { useRef, useState } from "react";
import { useDialogFocus } from "../components/use-dialog-focus";
import { IconClose } from "../icons";
import { normalizeWorkspacePath } from "../app/workspaces";
import { useI18n } from "../i18n";

interface CreateWorkspaceModalProps {
  onCreated: (path: string, name?: string) => void;
  onCancel: () => void;
}

export function CreateWorkspaceModal({
  onCreated,
  onCancel,
}: CreateWorkspaceModalProps) {
  const { t } = useI18n();
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useDialogFocus(root, onCancel);

  const handleSubmit = (e?: React.FormEvent) => {
    e?.preventDefault();
    const normalized = normalizeWorkspacePath(path);
    if (!normalized) {
      setError(t("workspaces.invalidPath"));
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
          <h2 id="create-workspace-title">{t("workspaces.newWorkspace")}</h2>
          <button
            type="button"
            className="icon-btn close-btn"
            onClick={onCancel}
            title={t("common.close")}
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
                {t("workspaces.pathLabel")} <span style={{ color: "var(--danger)" }}>*</span>
              </label>
              <input
                id="workspace-path-input"
                className="input mono"
                placeholder={t("workspaces.pathPlaceholder")}
                value={path}
                onChange={(e) => {
                  setPath(e.target.value);
                  if (error) setError(null);
                }}
                autoFocus
              />
              <div className="hint">
                {t("workspaces.pathHint")}
              </div>
            </div>

            <div className="field">
              <label className="label" htmlFor="workspace-name-input">
                {t("workspaces.nameLabel")}
              </label>
              <input
                id="workspace-name-input"
                className="input"
                placeholder={t("workspaces.namePlaceholder")}
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </div>

            <div className="hint" style={{ lineHeight: 1.6 }}>
              {t("workspaces.scopeHint")}
            </div>
          </div>

          <div className="modal-actions">
            <button type="button" className="btn ghost" onClick={onCancel}>
              {t("common.cancel")}
            </button>
            <button
              type="submit"
              className="btn primary"
              disabled={!path.trim()}
            >
              {t("workspaces.createAndSwitch")}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
