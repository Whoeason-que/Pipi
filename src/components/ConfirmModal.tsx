import { useRef, useState } from "react";
import { useDialogFocus } from "./use-dialog-focus";
import {
  IconClose
} from "../icons";


export default function ConfirmModal({
  title,
  message,
  confirmLabel = "彻底删除",
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmLabel?: string;
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useDialogFocus(root, () => { if (!busy) onCancel(); });

  return (
    <div className="modal-backdrop" onClick={() => { if (!busy) onCancel(); }}>
      <div
        ref={root}
        className="modal confirm-modal"
        role="alertdialog"
        aria-modal="true"
        aria-busy={busy}
        aria-labelledby="confirm-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="confirm-title">{title}</h2>
          <button type="button" className="icon-btn close-btn" onClick={onCancel} title="取消" disabled={busy}>
            <IconClose />
          </button>
        </div>
        <div className="modal-section">
          <p className="confirm-message">{message}</p>
        </div>
        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onCancel} disabled={busy}>
            取消
          </button>
          <button
            type="button"
            className="btn danger"
            aria-busy={busy}
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm();
              } finally {
                setBusy(false);
                onCancel();
              }
            }}
          >
            {busy ? "删除中…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
