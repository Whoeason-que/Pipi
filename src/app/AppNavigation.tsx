import { useContext } from "react";
import { useI18n } from "../i18n";
import {
  IconAgent,
  IconChart,
  IconForum,
  IconFolder,
  IconGit,
  IconGear,
  IconPanel,
  IconSearch,
  IconSkill,
  IconSubagent,
  IconTerminal,
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
}: {
  navSection: NavSection;
  settingsOpen: boolean;
  onOpenSection: (section: NavSection) => void;
  onToggleSettings: () => void;
  running: Record<AgentSection, boolean>;
  activeWorkspace?: string;
}) {
  const { t } = useI18n();
  const isWorkspaceFiltered = activeWorkspace !== "all";

  return (
    <nav className="rail" aria-label={t("nav.primary")}>
      <div className="rail-sections">
        {(
          [
            ["agents", t("nav.agents"), IconAgent],
            ["subagents", t("nav.subagents"), IconSubagent],
            ["stats", t("nav.stats"), IconChart],
            ["files", t("app.filesNav"), IconFolder],
            ["git", t("app.gitNav"), IconGit],
            ["terminal", t("nav.terminal"), IconTerminal],
            ["search", t("common.search"), IconSearch],
            ["skills", t("nav.skills"), IconSkill],
            ["forum", t("app.forum"), IconForum],
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
            {section === "agents" && isWorkspaceFiltered && (
                <span
                  className="rail-filter-badge"
                  aria-label={t("app.workspaceFiltering")}
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
