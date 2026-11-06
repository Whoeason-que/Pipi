import { useState } from "react";
import { Dialog } from "radix-ui";
import { useI18n } from "../i18n";
import { IconClose } from "../icons";

/**
 * 删除确认框。焦点陷阱、背景 inert、滚动锁与 Escape/点外关闭由 Radix Dialog
 * 负责，视觉仍是 `.modal` / `.confirm-modal` 自有样式。
 */
export default function ConfirmModal({
  title,
  message,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmLabel?: string;
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  // 没有 Dialog.Trigger 时 Radix 不会自动把焦点还给打开它的按钮；在首次渲染
  // （Radix 把焦点移进弹窗之前）记下当时的 activeElement，关闭时还回去。
  const [returnFocus] = useState<HTMLElement | null>(() =>
    typeof document !== "undefined" && document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null,
  );
  const closeBlocked = (event: Event) => {
    if (busy) event.preventDefault();
  };

  return (
    <Dialog.Root open onOpenChange={(open) => { if (!open && !busy) onCancel(); }}>
      <Dialog.Portal>
        <Dialog.Overlay className="modal-backdrop" />
        <Dialog.Content
          className="modal confirm-modal"
          role="alertdialog"
          aria-busy={busy}
          onEscapeKeyDown={closeBlocked}
          onPointerDownOutside={closeBlocked}
          onInteractOutside={closeBlocked}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            if (returnFocus?.isConnected) returnFocus.focus();
          }}
        >
          <div className="modal-header">
            <Dialog.Title>{title}</Dialog.Title>
            <button type="button" className="icon-btn close-btn" onClick={onCancel} title={t("common.cancel")} disabled={busy}>
              <IconClose />
            </button>
          </div>
          <div className="modal-section">
            <Dialog.Description className="confirm-message">{message}</Dialog.Description>
          </div>
          <div className="modal-actions">
            <button type="button" className="btn ghost" onClick={onCancel} disabled={busy}>
              {t("common.cancel")}
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
              {busy ? t("components.deleting") : confirmLabel ?? t("components.deletePermanently")}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
