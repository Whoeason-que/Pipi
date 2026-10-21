import type { ReactNode } from "react";

export type InspectorTab = "detail" | "stats" | "state" | "files" | "search";

export const INSPECTOR_TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: "detail", label: "调用详情" },
  { id: "stats", label: "会话统计" },
  { id: "state", label: "当前状态" },
  { id: "files", label: "会话文件" },
  { id: "search", label: "会话搜索" },
];

export function getInspectorTabLabel(
  tab: InspectorTab,
  t?: (key: string) => string,
): string {
  return t
    ? t(`inspector.${tab}`)
    : (INSPECTOR_TABS.find((item) => item.id === tab)?.label ?? tab);
}

export function InspectorTabIcon({
  tab,
  className,
}: {
  tab: InspectorTab;
  className?: string;
}) {
  const paths: Record<InspectorTab, ReactNode> = {
    detail: <path d="m4 5 6 6-6 6M12 19h8" />,
    stats: <path d="M18 20V10M12 20V4M6 20v-6" />,
    state: <path d="M22 12h-4l-3 9L9 3l-3 9H2" />,
    files: (
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8ZM14 2v6h6M8 13h8M8 17h8" />
    ),
    search: (
      <>
        <circle cx="11" cy="11" r="7" />
        <path d="m20 20-3.5-3.5" />
      </>
    ),
  };
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[tab]}
    </svg>
  );
}
