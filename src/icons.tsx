// 手写线性 SVG 图标（24 viewBox · stroke 1.6px）——全应用共用一套图标。
// 不引第三方图标库（AGENTS.md 依赖白名单）：图标少、风格统一、可 grep。
// 用法：<button className="icon-btn"><IconPlus/></button>，颜色跟随 currentColor。

import type { ReactNode } from "react";

function IconBase({
  children,
  size = 15,
  className,
}: {
  children: ReactNode;
  size?: number;
  className?: string;
}) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      className={className}
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

export function IconChevronRight({
  expanded = false,
}: { expanded?: boolean } = {}) {
  return (
    <IconBase
      size={14}
      className={`disclosure-chevron${expanded ? " open" : ""}`}
    >
      <path d="m9 6 6 6-6 6" />
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

export function IconMore() {
  return (
    <IconBase>
      <circle cx="5" cy="12" r="1" />
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
    </IconBase>
  );
}

export function IconArrowDown() {
  return (
    <IconBase>
      <path d="M12 4v16m-6-6 6 6 6-6" />
    </IconBase>
  );
}

export function IconArrowUp() {
  return (
    <IconBase>
      <path d="M12 20V4m-6 6 6-6 6 6" />
    </IconBase>
  );
}

export function IconPanel() {
  return (
    <IconBase>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M9 4v16" />
    </IconBase>
  );
}

/** 右侧面板开关（信息/引用/上下文） */
export function IconPanelRight() {
  return (
    <IconBase>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M15 4v16" />
    </IconBase>
  );
}

/** Pipi 的双环标记：在导航与空会话里复用。 */
export function PipiMark() {
  return (
    <svg viewBox="0 0 32 32" fill="none" aria-hidden="true">
      <path
        d="M7 25V10a6 6 0 1 1 6 6H7"
        stroke="currentColor"
        strokeWidth="3.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M19 25V15a4 4 0 1 1 4 4h-4"
        stroke="currentColor"
        strokeWidth="3.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function IconArrowUpRight() {
  return (
    <IconBase>
      <path d="M7 17 17 7M7 7h10v10" />
    </IconBase>
  );
}

export function IconFolder() {
  return (
    <IconBase>
      <path d="M3 7a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />
    </IconBase>
  );
}

/** 文件（预览标题、文件树） */
export function IconFile() {
  return (
    <IconBase>
      <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z" />
      <path d="M14 3v5h5" />
    </IconBase>
  );
}

/** 文件树展开箭头 */
export function IconCaret({ expanded = false }: { expanded?: boolean } = {}) {
  return (
    <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor"
      strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
      className={`file-caret${expanded ? " open" : ""}`}>
      <path d="m9 6 6 6-6 6" />
    </svg>
  );
}

export function IconMessage() {
  return (
    <IconBase>
      <path d="M21 11.5a8.5 8.5 0 0 1-8.5 8.5 9 9 0 0 1-4-.9L3 21l1.9-5.5a9 9 0 0 1-.9-4A8.5 8.5 0 0 1 12.5 3h.5a8.5 8.5 0 0 1 8 8v.5Z" />
      <path d="M8 10h9M8 14h5" />
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

/** Agent（左栏分区：普通 Agent） */
export function IconAgent() {
  return (
    <IconBase>
      <rect x="4" y="8.5" width="16" height="11" rx="3" />
      <path d="M12 4.6v3.9" />
      <circle cx="12" cy="3.4" r="1.3" />
      <path d="M9.2 13.6h.01M14.8 13.6h.01" />
    </IconBase>
  );
}

/** Subagent（左栏分区：Agent 组合工具创建的 child） */
export function IconSubagent() {
  return (
    <IconBase>
      <rect x="9" y="3" width="6" height="5" rx="1.6" />
      <rect x="3" y="16" width="6" height="5" rx="1.6" />
      <rect x="15" y="16" width="6" height="5" rx="1.6" />
      <path d="M12 8v4M6 16v-2h12v2" />
    </IconBase>
  );
}

/** 搜索（搜索分区 / 输入框内放大镜） */
/** 论坛（对话气泡） */
export function IconForum() {
  return (
    <IconBase>
      <path d="M20 15a3 3 0 0 1-3 3H9l-4 3v-3H7a3 3 0 0 1-3-3V7a3 3 0 0 1 3-3h10a3 3 0 0 1 3 3z" />
    </IconBase>
  );
}

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

/** 检查器（右侧详情面板） */
export function IconGrid() {
  return (
    <IconBase>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M15 4v16" />
    </IconBase>
  );
}

/** 发送（composer 主按钮） */
export function IconSend() {
  return (
    <IconBase size={13}>
      <path d="M12 20V4m-7 7 7-7 7 7" />
    </IconBase>
  );
}

/** 停止（运行中） */
export function IconStop({ size = 11 }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <rect x="6" y="6" width="12" height="12" rx="1.5" />
    </IconBase>
  );
}

/** 复制（消息操作） */
export function IconCopy() {
  return (
    <IconBase size={13}>
      <rect x="9" y="9" width="13" height="13" rx="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </IconBase>
  );
}

/** 已复制（消息操作的瞬时反馈） */
export function IconCheck() {
  return (
    <IconBase size={13}>
      <path d="M20 6L9 17l-5-5" />
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
/** 技能（能力包火花） */
export function IconSkill() {
  return (
    <IconBase>
      <path d="M11 3.5l1.7 4.6 4.6 1.7-4.6 1.7-1.7 4.6-1.7-4.6L4.7 9.8l4.6-1.7z" />
      <path d="M17.6 14.6l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z" />
    </IconBase>
  );
}

/** 刷新 / 重新读取 */
export function IconRefresh() {
  return (
    <IconBase>
      <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
      <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
      <path d="M8 16H3v5" />
    </IconBase>
  );
}

/** 安装（装入全局技能 store） */
export function IconDownload() {
  return (
    <IconBase>
      <path d="M12 4v10" />
      <path d="M8 10.5l4 4 4-4" />
      <path d="M5 19.5h14" />
    </IconBase>
  );
}

/** 警告三角形（通用错误） */
export function IconAlertTriangle({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
      <line x1="12" y1="9" x2="12" y2="13" />
      <line x1="12" y1="17" x2="12.01" y2="17" />
    </IconBase>
  );
}

/** 钥匙（认证 / API Key 异常） */
export function IconKey({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <circle cx="7.5" cy="15.5" r="5.5" />
      <path d="m21 2-9.6 9.6M15.5 7.5l3 3M18.5 4.5l3 3" />
    </IconBase>
  );
}

/** 时钟（速率限制 / 频次耗尽） */
export function IconClock({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <circle cx="12" cy="12" r="10" />
      <polyline points="12 6 12 12 16 14" />
    </IconBase>
  );
}

/** 断网 / 连接中断 */
export function IconWifiOff({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <line x1="1" y1="1" x2="23" y2="23" />
      <path d="M16.72 11.06A10.94 10.94 0 0 1 19 12.55" />
      <path d="M5 12.55a10.94 10.94 0 0 1 5.17-2.39" />
      <path d="M10.71 5.05A16 16 0 0 1 22.58 9" />
      <path d="M1.42 9a15.91 15.91 0 0 1 4.7-2.88" />
      <path d="M8.53 16.11a6 6 0 0 1 6.95 0" />
      <line x1="12" y1="20" x2="12.01" y2="20" />
    </IconBase>
  );
}

/** 首页 / 工作区 */
export function IconHome({ size }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <path d="M3 9.5L12 3l9 6.5V20a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V9.5z" />
      <path d="M9 22V12h6v10" />
    </IconBase>
  );
}

/** 图层 / 上下文窗口 */
export function IconLayers({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <polygon points="12 2 2 7 12 12 22 7 12 2" />
      <polyline points="2 17 12 22 22 17" />
      <polyline points="2 12 12 17 22 12" />
    </IconBase>
  );
}

/** 盾牌告警（权限拒绝 / 安全策略） */
export function IconShieldAlert({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
      <line x1="12" y1="8" x2="12" y2="12" />
      <line x1="12" y1="16" x2="12.01" y2="16" />
    </IconBase>
  );
}

/** 协议切换 / 双向循环 */
export function IconArrowRepeat({ size }: { size?: number }) {
  return (
    <IconBase size={size}>
      <path d="m17 2 4 4-4 4" />
      <path d="M3 11v-1a4 4 0 0 1 4-4h14" />
      <path d="m7 22-4-4 4-4" />
      <path d="M21 13v1a4 4 0 0 1-4 4H3" />
    </IconBase>
  );
}

/** 工作区源码管理 */
export function IconGit() {
  return <IconBase><circle cx="6" cy="5" r="3" /><circle cx="18" cy="6" r="3" /><circle cx="6" cy="19" r="3" /><path d="M6 8v8m12-7c0 5-12 1-12 7" /></IconBase>;
}

export function IconGitBranch({ size = 14 }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <line x1="6" y1="3" x2="6" y2="15" />
      <circle cx="18" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <path d="M18 9a9 9 0 0 1-9 9" />
    </IconBase>
  );
}

export function IconTree({ size = 15 }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <path d="M4 5h5v4H4zM13 9h7v4h-7zM13 16h7v4h-7z" />
      <path d="M9 7h2a2 2 0 0 1 2 2v9M13 11h-2M13 18h-2" />
    </IconBase>
  );
}

export function IconList({ size = 15 }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <line x1="8" y1="6" x2="20" y2="6" />
      <line x1="8" y1="12" x2="20" y2="12" />
      <line x1="8" y1="18" x2="20" y2="18" />
      <line x1="4" y1="6" x2="4.01" y2="6" />
      <line x1="4" y1="12" x2="4.01" y2="12" />
      <line x1="4" y1="18" x2="4.01" y2="18" />
    </IconBase>
  );
}

export function IconCollapseAll({ size = 14 }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <path d="M4 8h16M4 16h16M12 4v4m0 8v4" />
    </IconBase>
  );
}

/** 用户 / 人类 */
export function IconUser({ size }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <circle cx="12" cy="7.5" r="3.5" />
      <path d="M5.5 19.5c0-3.6 3-5.5 6.5-5.5s6.5 1.9 6.5 5.5" />
    </IconBase>
  );
}

/** 锁定（已关闭） */
export function IconLock({ size }: { size?: number } = {}) {
  return (
    <IconBase size={size}>
      <rect x="5" y="11" width="14" height="10" rx="2" />
      <path d="M8 11V7a4 4 0 0 1 8 0v4" />
    </IconBase>
  );
}

