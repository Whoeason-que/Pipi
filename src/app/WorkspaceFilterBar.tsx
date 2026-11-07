// Agent 侧栏顶部的工作区切换器：工作区过滤从独立页面降级后的紧凑入口。
// 外观对齐 .sidebar-quick（同一套高度、留白与描边），选择器复用 ChoiceSelect。
// 纯受控组件：value 跟随 active，不做任何内部状态；"all" 与具体路径都走 onSelect。
import { ChoiceSelect } from "../Select";
import { IconFolder } from "../icons";
import { useI18n } from "../i18n";
import type { WorkspaceInfo } from "./workspaces";

export interface WorkspaceFilterBarProps {
  /** 可选工作区（path/name/agentCount/custom）。 */
  workspaces: WorkspaceInfo[];
  /** 当前值："all" 或归一化路径。 */
  active: string;
  /** 选择回调，"all" 也走这里。 */
  onSelect: (path: string) => void;
}

export default function WorkspaceFilterBar({
  workspaces,
  active,
  onSelect,
}: WorkspaceFilterBarProps) {
  const { t } = useI18n();
  const choices = [
    { value: "all", label: t("app.workspaceFilterAll") },
    ...workspaces.map((workspace) => ({
      value: workspace.path,
      // 有关联 Agent 时追加 ` · {count}`，为 0 不加
      label:
        workspace.agentCount > 0
          ? `${workspace.name} · ${workspace.agentCount}`
          : workspace.name,
    })),
  ];
  return (
    <div className="workspace-filter">
      <IconFolder />
      <span className="workspace-filter-label">
        {t("app.workspaceFilterLabel")}
      </span>
      <ChoiceSelect
        className="workspace-filter-select"
        ariaLabel={t("app.workspaceFilterLabel")}
        value={active}
        choices={choices}
        onChange={onSelect}
        searchable={false}
        menuInPortal
        menuPlacement="auto"
      />
    </div>
  );
}
