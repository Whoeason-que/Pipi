// 侧栏顶部统一行（C 形态）：左「工作区 pill」，右「搜索槽」。
// pill 走 Radix 下拉（定位 / 碰撞翻转 / 键盘导航），外观自有：菜单行是
// 「标题 + 灰色描述行 + 选中对勾」。工作区语义（全局过滤 vs 浏览目标）由
// 调用方通过 value/onSelect 决定，这里只负责呈现与选择；右侧搜索槽由
// children 注入（agents/subagents 为 ⌘K 图标按钮，files/git 为局部过滤框）。
import { useMemo, type ReactNode } from "react";
import { DropdownMenu } from "radix-ui";
import { useI18n } from "../i18n";
import { IconCheck, IconChevronDown, IconFolder } from "../icons";
import { getWorkspaceBasename, type WorkspaceInfo } from "./workspaces";

/** 下拉一行：标题 + 灰色描述行 + 选中对勾。 */
interface ScopeMenuOption {
  value: string;
  label: string;
  description?: string;
  title?: string;
}

/** 共享的 scope 菜单内容：工作区 pill 与 Git 仓库选择器共用同一套行样式。 */
export function ScopeRadioMenu({
  options,
  value,
  onSelect,
  ariaLabel,
}: {
  options: ScopeMenuOption[];
  value: string;
  onSelect: (value: string) => void;
  ariaLabel: string;
}) {
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.Content
        className="scope-menu"
        aria-label={ariaLabel}
        align="start"
        sideOffset={4}
        collisionPadding={8}
      >
        <DropdownMenu.RadioGroup value={value} onValueChange={onSelect}>
          {options.map((option) => (
            <DropdownMenu.RadioItem
              key={option.value}
              value={option.value}
              className="scope-menu-item"
              title={option.title}
            >
              <span className="scope-menu-text">
                <span className="scope-menu-title">{option.label}</span>
                {option.description && <span className="scope-menu-desc">{option.description}</span>}
              </span>
              <DropdownMenu.ItemIndicator className="scope-menu-check">
                <IconCheck />
              </DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
          ))}
        </DropdownMenu.RadioGroup>
      </DropdownMenu.Content>
    </DropdownMenu.Portal>
  );
}

export function WorkspaceScopeBar({
  workspaces,
  value,
  onSelect,
  ariaLabel,
  includeAll = false,
  placeholder,
  children,
}: {
  workspaces: WorkspaceInfo[];
  /** 当前值："all"（需 includeAll）或归一化路径。 */
  value: string;
  onSelect: (value: string) => void;
  ariaLabel: string;
  /** 菜单是否包含 "all" 选项（agents/subagents 的过滤语义）。 */
  includeAll?: boolean;
  /** value 不在选项中（或没有可选项）时的显示文本。 */
  placeholder?: string;
  /** 右侧搜索槽。 */
  children: ReactNode;
}) {
  const { t } = useI18n();
  const options = useMemo((): ScopeMenuOption[] => {
    const count = (n: number) => t(n === 1 ? "workspace.groupAgentCountOne" : "workspace.groupAgentCount", { count: n });
    const rows: ScopeMenuOption[] = workspaces.map((workspace) => ({
      value: workspace.path,
      label: workspace.name || getWorkspaceBasename(workspace.path),
      description: count(workspace.agentCount),
      title: workspace.path,
    }));
    if (!includeAll) return rows;
    const total = workspaces.reduce((sum, workspace) => sum + workspace.agentCount, 0);
    return [
      { value: "all", label: t("app.workspaceFilterAll"), description: count(total) },
      ...rows,
    ];
  }, [workspaces, includeAll, t]);
  const current = options.find((option) => option.value === value);
  const label = current?.label ?? placeholder ?? value ?? "";
  return (
    <div className="workspace-scope">
      {options.length > 0 ? (
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              className="workspace-pill"
              aria-label={ariaLabel}
              title={current?.title ?? ariaLabel}
            >
              <IconFolder />
              <span className="workspace-pill-label">{label}</span>
              <span className="workspace-pill-chevron">
                <IconChevronDown />
              </span>
            </button>
          </DropdownMenu.Trigger>
          <ScopeRadioMenu options={options} value={value} onSelect={onSelect} ariaLabel={ariaLabel} />
        </DropdownMenu.Root>
      ) : (
        <div className="workspace-pill static" title={placeholder}>
          <IconFolder />
          <span className="workspace-pill-label">{label}</span>
        </div>
      )}
      {children}
    </div>
  );
}
