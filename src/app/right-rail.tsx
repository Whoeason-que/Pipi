import { useEffect, useRef, useState, type ReactNode } from "react";
import { IconClose } from "../icons";
import { useResizableWidth, type ResizableHandleProps } from "../resizable";

/** 右栏在窄屏改为抽屉；与左侧导航互斥地占用屏幕边缘。 */
export const RIGHT_RAIL_BREAKPOINT = 1180;

export interface RightRailController {
  open: boolean;
  setOpen: (open: boolean) => void;
  toggle: () => void;
  close: () => void;
  narrow: boolean;
  width: number;
  handleProps: ResizableHandleProps;
}

function readStoredOpen(key: string, fallback: boolean): boolean {
  if (typeof window === "undefined") return fallback;
  try {
    const value = window.localStorage.getItem(`pipi:right-rail:${key}:open`);
    return value === null ? fallback : value === "true";
  } catch {
    return fallback;
  }
}

/**
 * 右栏的通用状态：宽度、开合、窄屏抽屉与本地记忆。
 * 页面只负责“放什么”，不重复实现拖拽、Esc 与断点逻辑。
 */
export function useRightRail(key: string, defaultOpen = false): RightRailController {
  const [narrow, setNarrow] = useState(() => typeof window !== "undefined" && window.innerWidth <= RIGHT_RAIL_BREAKPOINT);
  const [open, setOpenState] = useState(() => {
    const currentlyNarrow = typeof window !== "undefined" && window.innerWidth <= RIGHT_RAIL_BREAKPOINT;
    return currentlyNarrow ? false : readStoredOpen(key, defaultOpen);
  });
  const resize = useResizableWidth({
    storageKey: `right-rail-${key}-width`,
    initial: 320,
    min: 268,
    max: 460,
    edge: "left",
  });
  const openRef = useRef(open);
  openRef.current = open;

  const setOpen = (next: boolean) => {
    setOpenState(next);
    try { window.localStorage.setItem(`pipi:right-rail:${key}:open`, String(next)); } catch { /* 存不下不影响本次使用 */ }
  };
  const close = () => setOpen(false);
  const toggle = () => setOpen(!openRef.current);

  useEffect(() => {
    const onResize = () => {
      const next = window.innerWidth <= RIGHT_RAIL_BREAKPOINT;
      setNarrow(next);
      if (next && openRef.current) close();
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    if (!open || !narrow) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !document.querySelector('[aria-modal="true"]')) close();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [narrow, open]);

  return { open, setOpen, toggle, close, narrow, width: resize.width, handleProps: resize.handleProps };
}

export interface RightRailTab {
  id: string;
  label: string;
  icon?: ReactNode;
  badge?: ReactNode;
  content: ReactNode;
}

export interface RightRailProps {
  id?: string;
  label: string;
  subtitle?: ReactNode;
  icon?: ReactNode;
  controller: RightRailController;
  tabs?: RightRailTab[];
  activeTab?: string;
  onTabChange?: (id: string) => void;
  children?: ReactNode;
  className?: string;
  actions?: ReactNode;
}

/** 全局右栏外壳：页面只声明标题、标签与内容。 */
export function RightRail({
  id,
  label,
  subtitle,
  icon,
  controller,
  tabs = [],
  activeTab,
  onTabChange,
  children,
  className = "",
  actions,
}: RightRailProps) {
  const [internalTab, setInternalTab] = useState<string>();
  const selectedId = activeTab ?? internalTab ?? tabs[0]?.id;
  const selected = tabs.find((tab) => tab.id === selectedId) ?? tabs[0];
  const body = tabs.length > 0 ? selected?.content : children;
  const changeTab = (id: string) => {
    setInternalTab(id);
    onTabChange?.(id);
  };
  return <>
    {controller.open && !controller.narrow && (
      <div
        className="resize-handle right-rail-resize"
        title="拖动调整右侧面板宽度（←/→ 微调）"
        aria-label="调整右侧面板宽度"
        {...controller.handleProps}
      />
    )}
    <aside
      id={id}
      inert={!controller.open}
      aria-hidden={!controller.open || undefined}
      className={`right-rail${controller.open ? " open" : " closed"} ${className}`.trim()}
      aria-label={label}
    >
      <header className="right-rail-head">
        {icon && <span className="right-rail-head-icon">{icon}</span>}
        <div className="right-rail-heading">
          <strong>{label}</strong>
          {subtitle && <small>{subtitle}</small>}
        </div>
        <div className="right-rail-actions">{actions}</div>
        <button type="button" className="icon-btn" title="关闭右侧面板" aria-label="关闭右侧面板" onClick={controller.close}>
          <IconClose />
        </button>
      </header>
      {tabs.length > 1 && (
        <div className="right-rail-tabs" role="tablist" aria-label={`${label}内容`}>
          {tabs.map((tab) => (
            <button key={tab.id} type="button" role="tab" aria-selected={tab.id === selected?.id}
              className={`right-rail-tab${tab.id === selected?.id ? " active" : ""}`}
              onClick={() => changeTab(tab.id)}>
              {tab.icon}{tab.label}{tab.badge !== undefined && <span className="right-rail-badge">{tab.badge}</span>}
            </button>
          ))}
        </div>
      )}
      <div className="right-rail-body">{body}</div>
    </aside>
    {controller.open && controller.narrow && (
      <button type="button" className="right-rail-backdrop" aria-label="关闭右侧面板" onClick={controller.close} />
    )}
  </>;
}
