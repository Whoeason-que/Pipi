// 手写线性 SVG 图标（24 viewBox · stroke 1.6px）——全应用共用一套图标。
// 不引第三方图标库（AGENTS.md 依赖白名单）：图标少、风格统一、可 grep。
// 用法：<button className="icon-btn"><IconPlus/></button>，颜色跟随 currentColor。

import type { ReactNode } from "react";

function IconBase({
  children,
  size = 15,
}: {
  children: ReactNode;
  size?: number;
}) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

/** 返回（顶栏 / 详情页） */
export function IconBack() {
  return (
    <IconBase>
      <path d="M15 18l-6-6 6-6" />
    </IconBase>
  );
}

/** 关闭（错误条 / 弹窗） */
export function IconClose() {
  return (
    <IconBase>
      <path d="M6 6l12 12M18 6L6 18" />
    </IconBase>
  );
}

/** 菜单（窄屏侧栏开关） */
export function IconMenu() {
  return (
    <IconBase>
      <path d="M4 7h16M4 12h16M4 17h16" />
    </IconBase>
  );
}

/** 设置 */
export function IconGear() {
  return (
    <IconBase>
      <circle cx="12" cy="12" r="3.2" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.03 1.56V21a2 2 0 1 1-4 0v-.09a1.7 1.7 0 0 0-1.11-1.56 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1.03H3a2 2 0 1 1 0-4h.09a1.7 1.7 0 0 0 1.56-1.11 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34h.08a1.7 1.7 0 0 0 1.03-1.56V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87v.08a1.7 1.7 0 0 0 1.56 1.03H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.56 1.03z" />
    </IconBase>
  );
}

/** 搜索（侧栏筛选） */
export function IconSearch() {
  return (
    <IconBase>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.5-3.5" />
    </IconBase>
  );
}

/** 新建（Agent / 会话） */
export function IconPlus() {
  return (
    <IconBase>
      <path d="M12 5v14M5 12h14" />
    </IconBase>
  );
}

/** 分叉会话 */
export function IconFork() {
  return (
    <IconBase>
      <path d="M6 3v12M6 15c0 3 2.5 5 5.5 5M6 15c0-3 3-4 5-4s4-1 4-4V3" />
      <path d="M9 6h8" />
      <path d="M14 3l3 3-3 3" />
    </IconBase>
  );
}

/** 检查器（九宫格） */
export function IconGrid() {
  return (
    <IconBase>
      <path d="M4 4h4v4H4zM10 4h4v4h-4zM16 4h4v4h-4zM4 10h4v4H4zM10 10h4v4h-4zM16 10h4v4h-4zM4 16h4v4H4zM10 16h4v4h-4zM16 16h4v4h-4z" />
    </IconBase>
  );
}

/** 发送（composer 主按钮） */
export function IconSend() {
  return (
    <IconBase size={13}>
      <path d="M22 2L11 13" />
      <path d="M22 2l-7 20-4-9-9-4 20-7z" />
    </IconBase>
  );
}

/** 停止（运行中） */
export function IconStop() {
  return (
    <IconBase size={11}>
      <rect x="6" y="6" width="12" height="12" rx="1.5" />
    </IconBase>
  );
}

/** 归档（移入箱子） */
export function IconArchive() {
  return (
    <IconBase>
      <path d="M4 5h16v4H4z" />
      <path d="M5 9v10h14V9" />
      <path d="M12 13v5M9.5 15.5 12 18l2.5-2.5" />
    </IconBase>
  );
}

/** 恢复（从箱子取出） */
export function IconRestore() {
  return (
    <IconBase>
      <path d="M4 5h16v4H4z" />
      <path d="M5 9v10h14V9" />
      <path d="M12 18v-5M9.5 15.5 12 13l2.5 2.5" />
    </IconBase>
  );
}

/** 删除（垃圾桶） */
export function IconTrash() {
  return (
    <IconBase>
      <path d="M4 6h16" />
      <path d="M9 6V4h6v2" />
      <path d="M6 6l1 14h10l1-14" />
      <path d="M10 10v6M14 10v6" />
    </IconBase>
  );
}