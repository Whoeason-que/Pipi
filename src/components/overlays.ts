/**
 * 当前是否有浮层占用屏幕：手写模态（`aria-modal`）、Radix Dialog/Popover
 * （`role="dialog"` + `data-state="open"`）。页面级的快捷键与 Escape 处理器
 * 先问它，再把按键让给浮层自己。
 *
 * Radix 关闭浮层时会 `preventDefault()`，所以 Escape 处理器还应同时检查
 * `event.defaultPrevented` —— 菜单与 popover 不带 `aria-modal`，这条是它们的
 * 唯一信号。
 */
const OVERLAY_SELECTOR =
  '[aria-modal="true"], [data-state="open"][role="dialog"], [data-state="open"][role="alertdialog"]';

export function overlayOpen(): boolean {
  return document.querySelector(OVERLAY_SELECTOR) !== null;
}
