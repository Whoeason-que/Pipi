import {
  IconAgent,
  IconGear,
  IconSearch,
  IconSkill,
  IconSubagent
} from "../icons";

/**
 * 左侧图标栏的分区：Agent / Subagent / 搜索 / 技能各自一个侧栏面板，
 * 全局设置（settingsOpen）是主区里的一个视图，不在分区状态里。
 */
export type NavSection = "agents" | "subs" | "search" | "skills";

export interface RailProps {
  navSection: NavSection;
  settingsOpen: boolean;
  onOpenSection: (section: NavSection) => void;
  onToggleSettings: () => void;
}

/** 左侧图标栏：分区切换与全局设置入口。 */
export function Rail({ navSection, settingsOpen, onOpenSection, onToggleSettings }: RailProps) {
  return (
    <nav className="rail" aria-label="主导航">
      <button
        type="button"
        className={`icon-btn${!settingsOpen && navSection === "agents" ? " active" : ""}`}
        title="Agent 列表"
        aria-label="Agent 列表"
        aria-current={!settingsOpen && navSection === "agents" ? "page" : undefined}
        onClick={() => onOpenSection("agents")}
      >
        <IconAgent />
      </button>
      <button
        type="button"
        className={`icon-btn${!settingsOpen && navSection === "subs" ? " active" : ""}`}
        title="Subagent 列表"
        aria-label="Subagent 列表"
        aria-current={!settingsOpen && navSection === "subs" ? "page" : undefined}
        onClick={() => onOpenSection("subs")}
      >
        <IconSubagent />
      </button>
      <button
        type="button"
        className={`icon-btn${!settingsOpen && navSection === "search" ? " active" : ""}`}
        title="搜索"
        aria-label="搜索"
        aria-current={!settingsOpen && navSection === "search" ? "page" : undefined}
        onClick={() => onOpenSection("search")}
      >
        <IconSearch />
      </button>
      <button
        type="button"
        className={`icon-btn${!settingsOpen && navSection === "skills" ? " active" : ""}`}
        title="技能库"
        aria-label="技能库"
        aria-current={!settingsOpen && navSection === "skills" ? "page" : undefined}
        onClick={() => onOpenSection("skills")}
      >
        <IconSkill />
      </button>
      <span className="rail-spacer" />
      <button
        type="button"
        className={`icon-btn${settingsOpen ? " active" : ""}`}
        title={settingsOpen ? "关闭设置" : "设置"}
        aria-label={settingsOpen ? "关闭设置" : "打开设置"}
        aria-current={settingsOpen ? "page" : undefined}
        onClick={onToggleSettings}
      >
        <IconGear />
      </button>
    </nav>
  );
}
