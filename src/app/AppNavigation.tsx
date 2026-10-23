import { useContext } from "react";
import { useI18n } from "../i18n";
import {
  IconAgent,
  IconForum,
  IconFolder,
  IconGit,
  IconGear,
  IconHome,
  IconPanel,
  IconSearch,
  IconSkill,
  IconSubagent,
  PipiMark,
} from "../icons";
import { LayoutContext } from "./layout-context";
import type { AgentSection, NavSection } from "./navigation";

export default function AppNavigation({
  navSection,
  settingsOpen,
  onOpenSection,
  onToggleSettings,
  running,
  activeWorkspace = "all",
  activeWorkspaceName = null,
}: {
  navSection: NavSection;
  settingsOpen: boolean;
  onOpenSection: (section: NavSection) => void;
  onToggleSettings: () => void;
  running: Record<AgentSection, boolean>;
  activeWorkspace?: string;
  activeWorkspaceName?: string | null;
}) {
  const { t } = useI18n();
  const isWorkspaceFiltered = activeWorkspace !== "all";

  return (
    <nav className="rail" aria-label={t("nav.primary")}>
      <button
        type="button"
        className="rail-brand"
        aria-label={t("nav.home")}
        title="Pipi"
        onClick={() => onOpenSection("agents")}
      >
        <PipiMark />
      </button>
      <div className="rail-sections">
        {(
          [
            ["workspaces", isWorkspaceFiltered ? `工作区 · ${activeWorkspaceName || "已过滤"}` : "工作区 · 全部", IconHome],
            ["agents", t("nav.agents"), IconAgent],
            ["subagents", t("nav.subagents"), IconSubagent],
            ["files", "File · 文件", IconFolder],
            ["git", "Git · 改动", IconGit],
            ["search", t("common.search"), IconSearch],
            ["skills", t("nav.skills"), IconSkill],
            ["forum", "论坛", IconForum],
          ] as const
        ).map(([section, label, Icon]) => (
          <button
            key={section}
            type="button"
            className={`icon-btn${!settingsOpen && navSection === section ? " active" : ""}`}
            title={label}
            aria-label={label}
            aria-current={
              !settingsOpen && navSection === section ? "page" : undefined
            }
            onClick={() => onOpenSection(section)}
          >
            <Icon />
            {section === "workspaces" && isWorkspaceFiltered && (
              <span
                className="rail-filter-badge"
                aria-label="工作区过滤中"
              />
            )}
            {(section === "agents" || section === "subagents") &&
              running[section] && (
                <span
                  className="rail-running"
                  aria-label={t("common.running")}
                />
              )}
          </button>
        ))}
      </div>
      <span className="rail-spacer" />
      <button
        type="button"
        className={`icon-btn${settingsOpen ? " active" : ""}`}
        title={t("nav.settings")}
        aria-label={
          settingsOpen ? t("nav.closeSettings") : t("nav.openSettings")
        }
        onClick={onToggleSettings}
      >
        <IconGear />
      </button>
    </nav>
  );
}

export function NavigationToggle() {
  const layout = useContext(LayoutContext);
  const { t } = useI18n();
  return (
    <button
      type="button"
      className="icon-btn navigation-toggle"
      aria-label={t("nav.toggleNavigation")}
      aria-controls="primary-navigation"
      onClick={layout.toggleNavigation}
    >
      <IconPanel />
    </button>
  );
}
