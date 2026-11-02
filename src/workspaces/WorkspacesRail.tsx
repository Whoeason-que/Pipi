// 工作区右栏：当前范围摘要与所有工作区的快速切换。

import { useRightRail, RightRail } from "../app/right-rail";
import {
  getWorkspaceBasename,
  normalizeWorkspacePath,
  type WorkspaceInfo,
} from "../app/workspaces";
import { IconAgent, IconFolder, IconGit, IconHome, IconPlus } from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { useI18n } from "../i18n";

export function WorkspacesRail({
  controller,
  workspaces,
  activeWorkspace,
  agents,
  sessionsByAgent,
  onSelectWorkspace,
  onOpenCreateModal,
  onCreateAgent,
  onNavigateSection,
}: {
  controller: ReturnType<typeof useRightRail>;
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  agents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  onSelectWorkspace: (workspace: string) => void;
  onOpenCreateModal: () => void;
  onCreateAgent: (defaultWorkspace?: string) => void;
  onNavigateSection: (section: "agents" | "files" | "git") => void;
}) {
  const { t } = useI18n();
  const isAll = activeWorkspace === "all";
  const current = workspaces.find((workspace) => workspace.path === activeWorkspace);
  const displayName = isAll ? t("workspaces.allWorkspaces") : current?.name || getWorkspaceBasename(activeWorkspace);
  const currentNorm = normalizeWorkspacePath(activeWorkspace);
  const primaryAgents = agents.filter((agent) => !agent.subagent);
  const scopedAgents = isAll ? primaryAgents : primaryAgents.filter((agent) => normalizeWorkspacePath(agent.workspace) === currentNorm);
  const sessions = scopedAgents.reduce((total, agent) => total + (sessionsByAgent[agent.name]?.length ?? 0), 0);

  return <RightRail
    controller={controller}
    id="workspaces-rail"
    label={displayName}
    subtitle={isAll
      ? t("workspaces.railSubtitleAll", { workspaces: workspaces.length, agents: scopedAgents.length })
      : t("workspaces.railSubtitleWorkspace", { agents: scopedAgents.length })}
    icon={isAll ? <IconHome /> : <IconFolder />}
    actions={<button type="button" className="icon-btn" title={t("workspaces.newWorkspace")} aria-label={t("workspaces.newWorkspace")} onClick={onOpenCreateModal}>
      <IconPlus />
    </button>}
    tabs={[
      {
        id: "scope",
        label: t("workspaces.scope"),
        content: <Scope workspaceName={displayName} path={isAll ? t("workspaces.allWorkspaces") : activeWorkspace} isAll={isAll}
          workspaces={workspaces.length} agents={scopedAgents.length} sessions={sessions}
          onOpenCreateModal={onOpenCreateModal} onCreateAgent={onCreateAgent}
          onNavigateSection={onNavigateSection} />,
      },
      {
        id: "switch",
        label: t("workspaces.switch"),
        badge: workspaces.length,
        content: <WorkspaceList workspaces={workspaces} activeWorkspace={activeWorkspace}
          onSelectWorkspace={onSelectWorkspace} />,
      },
    ]}
  />;
}

function Scope({ workspaceName, path, isAll, workspaces, agents, sessions, onOpenCreateModal, onCreateAgent, onNavigateSection }: {
  workspaceName: string;
  path: string;
  isAll: boolean;
  workspaces: number;
  agents: number;
  sessions: number;
  onOpenCreateModal: () => void;
  onCreateAgent: (defaultWorkspace?: string) => void;
  onNavigateSection: (section: "agents" | "files" | "git") => void;
}) {
  const { t } = useI18n();
  return <>
    <section className="right-rail-section">
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        {isAll ? <IconHome /> : <IconFolder />}
        <span className="truncate right-rail-item-main">{workspaceName}</span>
      </div>
      <div className="right-rail-kv"><span>{t("workspaces.path")}</span><span className="mono">{path}</span></div>
      <div className="right-rail-kv"><span>{t("workspaces.scope")}</span><span>{isAll ? t("workspaces.allWorkspaces") : t("workspaces.singleWorkspace")}</span></div>
    </section>
    <section className="right-rail-section">
      <h3>{t("workspaces.overview")}</h3>
      <div className="right-rail-stats">
        <div className="right-rail-stat"><span>{t("workspaces.stats.workspaces")}</span><strong>{workspaces}</strong></div>
        <div className="right-rail-stat"><span>{t("workspaces.stats.agents")}</span><strong>{agents}</strong></div>
        <div className="right-rail-stat"><span>{t("workspaces.stats.sessions")}</span><strong>{sessions}</strong></div>
        <div className="right-rail-stat"><span>{t("workspaces.stats.mode")}</span><strong>{isAll ? t("workspaces.stats.overviewMode") : t("workspaces.stats.workspaceMode")}</strong></div>
      </div>
    </section>
    <section className="right-rail-section">
      <h3>{t("workspaces.quickActions")}</h3>
      <div className="right-rail-actions-row">
        <button type="button" className="btn small" onClick={onOpenCreateModal}><IconPlus /> {t("workspaces.newWorkspace")}</button>
        <button type="button" className="btn small ghost" onClick={() => onCreateAgent(isAll ? undefined : path)}>
          <IconAgent /> {t("nav.newAgent")}
        </button>
        {!isAll && <>
          <button type="button" className="btn small ghost" onClick={() => onNavigateSection("files")}><IconFolder /> File</button>
          <button type="button" className="btn small ghost" onClick={() => onNavigateSection("git")}><IconGit /> Git</button>
        </>}
        {isAll && <button type="button" className="btn small ghost" onClick={() => onNavigateSection("agents")}><IconAgent /> {t("workspaces.allAgents")}</button>}
      </div>
      <p className="caption right-rail-note">{t("workspaces.railNote")}</p>
    </section>
  </>;
}

function WorkspaceList({ workspaces, activeWorkspace, onSelectWorkspace }: {
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  onSelectWorkspace: (workspace: string) => void;
}) {
  const { t } = useI18n();
  return <section className="right-rail-section">
    <h3>{t("workspaces.allListTitleDot", { count: workspaces.length })}</h3>
    <div className="right-rail-list">
      <button type="button" className={`right-rail-item${activeWorkspace === "all" ? " active" : ""}`} onClick={() => onSelectWorkspace("all")}>
        <IconHome />
        <span className="truncate right-rail-item-main">{t("workspaces.allWorkspaces")}</span>
        <small>{t("workspaces.agentCount", { count: workspaces.reduce((total, item) => total + item.agentCount, 0) })}</small>
      </button>
      {workspaces.map((workspace) => <button type="button" key={workspace.path}
        className={`right-rail-item${workspace.path === activeWorkspace ? " active" : ""}`}
        title={workspace.path} onClick={() => onSelectWorkspace(workspace.path)}>
        <IconFolder />
        <span className="truncate right-rail-item-main">{workspace.name || getWorkspaceBasename(workspace.path)}</span>
        <small>{t("workspaces.agentCount", { count: workspace.agentCount })}</small>
      </button>)}
    </div>
  </section>;
}
