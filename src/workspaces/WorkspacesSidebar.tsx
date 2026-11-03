import { useContext, useMemo, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import {
  IconCheck,
  IconFolder,
  IconHome,
  IconPlus,
  IconTrash,
} from "../icons";
import { SidebarSearch } from "../components/SidebarSearch";
import { getWorkspaceBasename, type WorkspaceInfo } from "../app/workspaces";
import { useI18n } from "../i18n";

interface WorkspacesSidebarProps {
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  totalAgentsCount: number;
  onSelectWorkspace: (workspace: string) => void;
  onOpenCreateModal: () => void;
  onRemoveCustomWorkspace?: (path: string) => void;
  onViewAgents?: () => void;
  onCreateAgent?: (workspacePath?: string) => void;
}

export function WorkspacesSidebar({
  workspaces,
  activeWorkspace,
  totalAgentsCount,
  onSelectWorkspace,
  onOpenCreateModal,
  onRemoveCustomWorkspace,
  onViewAgents,
  onCreateAgent,
}: WorkspacesSidebarProps) {
  const { closeNavigation } = useContext(LayoutContext);
  const { t } = useI18n();
  const [filterQuery, setFilterQuery] = useState("");

  const filtered = useMemo(() => {
    const q = filterQuery.trim().toLowerCase();
    if (!q) return workspaces;
    return workspaces.filter((w) => {
      const name = (w.name || getWorkspaceBasename(w.path)).toLowerCase();
      const path = w.path.toLowerCase();
      return name.includes(q) || path.includes(q);
    });
  }, [workspaces, filterQuery]);

  const isAll = activeWorkspace === "all";

  const handleSelect = (path: string) => {
    onSelectWorkspace(path);
  };

  return (
    <>
      {/* 搜索置顶：过滤框作为侧栏第一行 */}
      <SidebarSearch
        value={filterQuery}
        onChange={setFilterQuery}
        placeholder={t("workspaces.searchPlaceholder")}
        label={t("workspaces.searchLabel")}
      />

      <div className="caption sb-sec workspaces-heading">
        <span>{t("workspaces.title")}</span>
        <span className="spacer" />
        <span className="caption sb-count">{workspaces.length}</span>
        <button
          type="button"
          className="icon-btn"
          title={t("workspaces.newWorkspace")}
          aria-label={t("workspaces.newWorkspace")}
          onClick={onOpenCreateModal}
        >
          <IconPlus />
        </button>
      </div>

      <div className="workspaces-sidebar-list" role="listbox" aria-label={t("workspaces.listLabel")}>
        {/* 全部工作区 (All) */}
        {(!filterQuery || "全部工作区 all".includes(filterQuery.toLowerCase())) && (
          <div
            className={`workspaces-nav-item${isAll ? " selected" : ""}`}
            role="option"
            aria-selected={isAll}
            onClick={() => handleSelect("all")}
          >
            <span className="icon-tile workspaces-nav-icon">
              <IconHome />
            </span>
            <div className="workspaces-nav-info">
              <div className="workspaces-nav-title-row">
                <span className="truncate workspaces-nav-name">{t("workspaces.allWorkspaces")}</span>
                {isAll && (
                  <span className="workspaces-nav-check" title={t("workspaces.selected")}>
                    <IconCheck />
                  </span>
                )}
              </div>
              <span className="truncate workspaces-nav-sub">{t(totalAgentsCount === 1 ? "workspaces.showAllAgentsOne" : "workspaces.showAllAgents", { count: totalAgentsCount })}</span>
            </div>
          </div>
        )}

        {filtered.map((w) => {
          const isSelected = activeWorkspace === w.path;
          const displayName = w.name || getWorkspaceBasename(w.path);
          const canDelete = Boolean(w.custom && w.agentCount === 0 && onRemoveCustomWorkspace);

          return (
            <div
              key={w.path}
              className={`workspaces-nav-item${isSelected ? " selected" : ""}`}
              role="option"
              aria-selected={isSelected}
              onClick={() => handleSelect(w.path)}
            >
              <span className="icon-tile workspaces-nav-icon">
                <IconFolder />
              </span>
              <div className="workspaces-nav-info">
                <div className="workspaces-nav-title-row">
                  <span className="truncate workspaces-nav-name" title={displayName}>
                    {displayName}
                  </span>
                  <span className="workspaces-nav-badge">{w.agentCount}</span>
                  {isSelected && (
                    <span className="workspaces-nav-check" title={t("workspaces.selected")}>
                      <IconCheck />
                    </span>
                  )}
                  {canDelete && (
                    <button
                      type="button"
                      className="workspaces-nav-delete"
                      title={t("workspaces.removeCustom")}
                      aria-label={t("workspaces.removeNamed", { name: displayName })}
                      onClick={(e) => {
                        e.stopPropagation();
                        onRemoveCustomWorkspace?.(w.path);
                      }}
                    >
                      <IconTrash />
                    </button>
                  )}
                </div>
                <span className="truncate workspaces-nav-sub" title={w.path}>
                  {w.path}
                </span>

                {isSelected && (
                  <div
                    className="workspaces-nav-actions"
                    onClick={(e) => e.stopPropagation()}
                  >
                    {onViewAgents && (
                      <button
                        type="button"
                        className="btn small"
                        onClick={() => {
                          closeNavigation();
                          onViewAgents();
                        }}
                      >
                        {t("workspaces.viewAgents")}
                      </button>
                    )}
                    {onCreateAgent && (
                      <button
                        type="button"
                        className="btn small primary"
                        onClick={() => {
                          closeNavigation();
                          onCreateAgent(w.path);
                        }}
                      >
                        + {t("nav.newAgent")}
                      </button>
                    )}
                  </div>
                )}
              </div>
            </div>
          );
        })}

        {filtered.length === 0 && filterQuery && (
          <div className="sb-empty">{t("workspaces.noMatches")}</div>
        )}
      </div>
    </>
  );
}
