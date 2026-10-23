import { useContext, useMemo, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import {
  IconCheck,
  IconClose,
  IconFolder,
  IconHome,
  IconPlus,
  IconSearch,
  IconTrash,
} from "../icons";
import { getWorkspaceBasename, type WorkspaceInfo } from "../app/workspaces";

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
      <div className="sb-sec workspaces-heading">
        <span>工作区</span>
        <span className="spacer" />
        <span className="sb-count">{workspaces.length}</span>
        <button
          type="button"
          className="icon-btn"
          title="新建工作区"
          aria-label="新建工作区"
          onClick={onOpenCreateModal}
        >
          <IconPlus />
        </button>
      </div>

      <div className="sb-filter">
        <div className="search">
          <IconSearch />
          <input
            value={filterQuery}
            onChange={(e) => setFilterQuery(e.target.value)}
            placeholder="搜索工作区…"
            aria-label="搜索工作区"
            autoComplete="off"
            spellCheck={false}
          />
          {filterQuery && (
            <button
              type="button"
              className="search-clear"
              title="清空搜索"
              onClick={() => setFilterQuery("")}
            >
              <IconClose />
            </button>
          )}
        </div>
      </div>

      <div className="workspaces-sidebar-list" role="listbox" aria-label="工作区列表">
        {/* 全部工作区 (All) */}
        {(!filterQuery || "全部工作区 all".includes(filterQuery.toLowerCase())) && (
          <div
            className={`workspaces-nav-item${isAll ? " selected" : ""}`}
            role="option"
            aria-selected={isAll}
            onClick={() => handleSelect("all")}
          >
            <span className="workspaces-nav-icon">
              <IconHome />
            </span>
            <div className="workspaces-nav-info">
              <div className="workspaces-nav-title-row">
                <span className="workspaces-nav-name">全部工作区 (All)</span>
                {isAll && (
                  <span className="workspaces-nav-check" title="当前已选择">
                    <IconCheck />
                  </span>
                )}
              </div>
              <span className="workspaces-nav-sub">显示全部 {totalAgentsCount} 个 Agent</span>
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
              <span className="workspaces-nav-icon">
                <IconFolder />
              </span>
              <div className="workspaces-nav-info">
                <div className="workspaces-nav-title-row">
                  <span className="workspaces-nav-name" title={displayName}>
                    {displayName}
                  </span>
                  <span className="workspaces-nav-badge">{w.agentCount}</span>
                  {isSelected && (
                    <span className="workspaces-nav-check" title="当前已选择">
                      <IconCheck />
                    </span>
                  )}
                  {canDelete && (
                    <button
                      type="button"
                      className="workspaces-nav-delete"
                      title="移除此自定义工作区"
                      aria-label={`移除 ${displayName}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        onRemoveCustomWorkspace?.(w.path);
                      }}
                    >
                      <IconTrash />
                    </button>
                  )}
                </div>
                <span className="workspaces-nav-sub" title={w.path}>
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
                        className="workspaces-inline-btn"
                        onClick={() => {
                          closeNavigation();
                          onViewAgents();
                        }}
                      >
                        查看 Agent
                      </button>
                    )}
                    {onCreateAgent && (
                      <button
                        type="button"
                        className="workspaces-inline-btn primary"
                        onClick={() => {
                          closeNavigation();
                          onCreateAgent(w.path);
                        }}
                      >
                        + 新建 Agent
                      </button>
                    )}
                  </div>
                )}
              </div>
            </div>
          );
        })}

        {filtered.length === 0 && filterQuery && (
          <div className="sb-empty">未匹配到工作区</div>
        )}
      </div>
    </>
  );
}
