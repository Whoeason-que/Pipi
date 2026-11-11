import { useEffect, useRef, type RefObject } from "react";

/**
 * 手写模态的焦点陷阱：Tab 在弹窗内循环、Escape 关闭、卸载时把焦点还给
 * 打开它的元素。`escapeGuard` 用于让内层控件先吃掉 Escape（例如 focus 还在
 * react-select 的菜单里时，这一下只该关菜单）。
 */
export function useDialogFocus(
  ref: RefObject<HTMLElement | null>,
  close: () => void,
  active = true,
  escapeGuard?: () => boolean,
) {
  const closeRef = useRef(close);
  closeRef.current = close;
  const guardRef = useRef(escapeGuard);
  guardRef.current = escapeGuard;
  // 打开前的焦点要在 render 阶段记下：弹窗里的 autoFocus 在提交阶段就会把焦点
  // 搬进弹窗，effect 里再读 activeElement 已经拿不到打开它的元素了。关闭时
  // （active 变 false）再清空，重新打开才能记到新的 opener。
  const openerRef = useRef<HTMLElement | null>(null);
  if (!active) openerRef.current = null;
  else if (!openerRef.current && typeof document !== "undefined" && document.activeElement instanceof HTMLElement) {
    openerRef.current = document.activeElement;
  }
  useEffect(() => {
    if (!active) return;
    const previous = openerRef.current;
    const dialog = ref.current;
    if (!dialog) return;
    const focusable = () => [...dialog.querySelectorAll<HTMLElement>(
      'button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]',
    )].filter(node => node.getClientRects().length > 0);
    (focusable()[0] ?? dialog).focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if (event.key === "Escape") {
        // 先问内层：菜单/下拉吃掉这一下时不再关闭整个弹窗。
        if (guardRef.current && !guardRef.current()) return;
        event.preventDefault();
        event.stopPropagation();
        closeRef.current();
      }
      if (event.key !== "Tab") return;
      const elements = focusable();
      const first = elements[0];
      const last = elements.at(-1);
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    dialog.addEventListener("keydown", keydown);
    return () => {
      dialog.removeEventListener("keydown", keydown);
      // StrictMode 会立刻重跑一次 effect；那时 dialog 仍可见，不能把焦点抢回
      // 背后的按钮。真正的关闭要么卸载，要么把容器移出 DOM / 置为 inert / 隐藏。
      const released = !dialog.isConnected
        || Boolean(dialog.closest("[inert]"))
        || dialog.getClientRects().length === 0;
      if (released && previous?.isConnected) previous.focus();
    };
  }, [ref, active]);
}
