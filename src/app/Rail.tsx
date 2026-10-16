import { IconAgent, IconGear, IconSearch, IconSkill } from "../icons";

export type NavSection = "agents" | "search" | "skills";

export function Rail({ navSection, settingsOpen, onOpenSection, onToggleSettings }: {
  navSection: NavSection; settingsOpen: boolean;
  onOpenSection: (section: NavSection) => void; onToggleSettings: () => void;
}) {
  return <nav className="rail" aria-label="主导航">
    {([["agents", "Agent 列表", IconAgent], ["search", "搜索", IconSearch], ["skills", "技能库", IconSkill]] as const).map(([section, label, Icon]) =>
      <button key={section} type="button" className={`icon-btn${!settingsOpen && navSection === section ? " active" : ""}`}
        title={label} aria-label={label} aria-current={!settingsOpen && navSection === section ? "page" : undefined}
        onClick={() => onOpenSection(section)}><Icon /></button>)}
    <span className="rail-spacer" />
    <button type="button" className={`icon-btn${settingsOpen ? " active" : ""}`} title="设置"
      aria-label={settingsOpen ? "关闭设置" : "打开设置"} onClick={onToggleSettings}><IconGear /></button>
  </nav>;
}
