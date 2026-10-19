import { useContext } from "react";
import { useI18n } from "../i18n";
import {
  IconAgent,
  IconForum,
  IconGear,
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
}: {
  navSection: NavSection;
  settingsOpen: boolean;
  onOpenSection: (section: NavSection) => void;
  onToggleSettings: () => void;
  running: Record<AgentSection, boolean>;
}) {
  const { t } = useI18n();
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
            ["agents", t("nav.agents"), IconAgent],
            ["subagents", t("nav.subagents"), IconSubagent],
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
