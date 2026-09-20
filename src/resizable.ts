import { useCallback, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";

/**
 * 可拖拽调宽的侧栏/检查器宽度。
 *
 * 宽度通过 CSS 自定义属性（`--sidebar-width` / `--inspector-width`）注入，
 * 而不是内联 width —— 窄屏媒体查询里的抽屉宽度才能正常覆盖它。拖拽用
 * pointer capture，键盘左右方向键同样可调（每次 16px），宽度存 localStorage。
 */
export interface ResizableOptions {
  /** localStorage 键：宽度跨会话保留。 */
  storageKey: string;
  /** 初始宽度（px）。 */
  initial: number;
  min: number;
  max: number;
  /** 手柄相对面板的位置："right"（面板在左，向右拖变宽）/ "left"（面板在右，向左拖变宽）。 */
  edge: "left" | "right";
}

export interface ResizableHandleProps {
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerUp: (event: ReactPointerEvent<HTMLElement>) => void;
  onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
  role: "separator";
  tabIndex: number;
  "aria-orientation": "vertical";
  "aria-valuenow": number;
  "aria-valuemin": number;
  "aria-valuemax": number;
}

const STORAGE_PREFIX = "pipi:";

/** 拖拽期间给 body 加类：保持 col-resize 指针形态并禁止选中文本。 */
function setResizing(active: boolean): void {
  if (typeof document === "undefined") return;
  document.body.classList.toggle("resizing", active);
}

function readStoredWidth(options: ResizableOptions): number {
  const clamp = (value: number) => Math.min(options.max, Math.max(options.min, value));
  if (typeof window === "undefined") return options.initial;
  try {
    const raw = window.localStorage.getItem(STORAGE_PREFIX + options.storageKey);
    const parsed = raw ? Number.parseInt(raw, 10) : Number.NaN;
    return Number.isFinite(parsed) ? clamp(parsed) : options.initial;
  } catch {
    return options.initial;
  }
}

export function useResizableWidth(options: ResizableOptions): {
  width: number;
  handleProps: ResizableHandleProps;
} {
  const [width, setWidth] = useState(() => readStoredWidth(options));
  const widthRef = useRef(width);
  widthRef.current = width;
  const dragRef = useRef<{ pointerId: number; startX: number; startWidth: number } | null>(null);

  const clamp = useCallback(
    (value: number) => Math.min(options.max, Math.max(options.min, value)),
    [options.max, options.min],
  );

  const commit = useCallback(
    (next: number) => {
      const clamped = clamp(next);
      widthRef.current = clamped;
      setWidth(clamped);
      try {
        window.localStorage.setItem(STORAGE_PREFIX + options.storageKey, String(clamped));
      } catch {
        // 存不下不影响本次使用
      }
    },
    [clamp, options.storageKey],
  );

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      dragRef.current = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startWidth: widthRef.current,
      };
      setResizing(true);
    },
    [],
  );

  const onPointerMove = useCallback(
    (event: ReactPointerEvent<HTMLElement>) => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      const delta = event.clientX - drag.startX;
      // 面板在左：右拖变宽；面板在右：左拖变宽
      const next = options.edge === "right" ? drag.startWidth + delta : drag.startWidth - delta;
      commit(next);
    },
    [commit, options.edge],
  );

  const onPointerUp = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    setResizing(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }, []);

  const onKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLElement>) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      const step = event.key === "ArrowRight" ? 16 : -16;
      const delta = options.edge === "right" ? step : -step;
      commit(widthRef.current + delta);
    },
    [commit, options.edge],
  );

  return {
    width,
    handleProps: {
      onPointerDown,
      onPointerMove,
      onPointerUp,
      onKeyDown,
      role: "separator",
      tabIndex: 0,
      "aria-orientation": "vertical",
      "aria-valuenow": Math.round(width),
      "aria-valuemin": options.min,
      "aria-valuemax": options.max,
    },
  };
}
