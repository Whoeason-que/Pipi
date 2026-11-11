import type { ReactNode } from "react";

export type InspectorTab = "notebook" | "detail" | "state" | "history" | "files" | "search" | "browser" | "terminal";

export const INSPECTOR_TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: "state", label: "State" },
  { id: "detail", label: "Details" },
  { id: "notebook", label: "Notebook" },
  { id: "files", label: "Session files" },
  { id: "history", label: "History" },
  { id: "search", label: "Find" },
  { id: "browser", label: "Browser" },
  { id: "terminal", label: "Terminal" },
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
    terminal: (
      <>
        <path d="m4 6 4 4-4 4" />
        <path d="M12 14h8" />
      </>
    ),
    browser: <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18" /></>,
    notebook: (
      <>
        <path d="M8 3h8a2 2 0 0 1 2 2v15a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z" />
        <path d="m9 11 1.5 1.5L13 10" />
        <path d="M9 16h6" />
      </>
    ),
    detail: <path d="m4 5 6 6-6 6M12 19h8" />,
    state: <path d="M22 12h-4l-3 9L9 3l-3 9H2" />,
    history: (
      <>
        <circle cx="6" cy="6" r="2.5" />
        <circle cx="6" cy="18" r="2.5" />
        <circle cx="18" cy="12" r="2.5" />
        <path d="M6 8.5v7M8.5 6H14a4 4 0 0 1 4 4v.5" />
      </>
    ),
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
