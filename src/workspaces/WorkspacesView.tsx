import { NavigationToggle } from "../app/AppNavigation";
import type { CSSProperties } from "react";
import { useRightRail } from "../app/right-rail";
import AgentAvatar from "../components/AgentAvatar";
import {
  IconAgent,
  IconFolder,
  IconGit,
  IconHome,
  IconPanelRight,
  IconPlus,
} from "../icons";
import {
  getWorkspaceBasename,
  normalizeWorkspacePath,
  type WorkspaceInfo,
} from "../app/workspaces";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { useI18n } from "../i18n";
import { WorkspacesRail } from "./WorkspacesRail";

interface WorkspacesViewProps {
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  agents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  onSelectWorkspace: (workspace: string) => void;
  onOpenCreateModal: () => void;
  onSelectAgent: (agentName: string) => void;
  onConfigureAgent: (agentName: string) => void;
  onOpenChat: (agentName: string, sessionId: string | null) => void;
  onCreateAgent: (defaultWorkspace?: string) => void;
  onNavigateSection: (section: "agents" | "subagents" | "files" | "git") => void;
}

export function WorkspacesView({
  workspaces,
  activeWorkspace,
  agents,
  sessionsByAgent,
  onSelectWorkspace,
  onOpenCreateModal,
  onSelectAgent,
  onConfigureAgent,
  onOpenChat,
  onCreateAgent,
  onNavigateSection,
}: WorkspacesViewProps) {
  const { t } = useI18n();
  const rail = useRightRail("workspaces");
  const isAll = activeWorkspace === "all";
  const currentWorkspace = workspaces.find((w) => w.path === activeWorkspace);
  const displayName = isAll
    ? t("workspaces.allWorkspaces")
    : currentWorkspace?.name || getWorkspaceBasename(activeWorkspace);

  // 属于当前工作区的 Agents（工作区只关注主 Agent，不看 Subagent）
  const currentNorm = normalizeWorkspacePath(activeWorkspace);
  const primaryAgents = agents.filter((a) => !a.subagent);
  const workspaceAgents = isAll
    ? primaryAgents
    : primaryAgents.filter(
        (a) => normalizeWorkspacePath(a.workspace) === currentNorm,
      );

  const totalSessionsCount = workspaceAgents.reduce(
    (acc, a) => acc + (sessionsByAgent[a.name]?.length ?? 0),
    0,
  );

  return (
    <div className="workspaces-view" style={{ "--right-rail-width": `${rail.width}px` } as CSSProperties}>
      <div className="content-pane workspaces-main">
      <header className="workspace-toolbar">
        <NavigationToggle />
        <span className="toolbar-divider" />
        <span className="workspaces-toolbar-title">
          {isAll
            ? t("workspaces.toolbarAll")
            : t("workspaces.toolbarNamed", { name: displayName })}
        </span>
        <span className="spacer" />
        <button
          type="button"
          className={`icon-btn${rail.open ? " active" : ""}`}
          title={t("workspaces.infoTitle")}
          aria-label={t("workspaces.toggleInfoPanel")}
          aria-pressed={rail.open}
          onClick={rail.toggle}
        >
          <IconPanelRight />
        </button>
        <button
          type="button"
          className="btn small"
          onClick={onOpenCreateModal}
        >
          <IconPlus />
          <span>{t("workspaces.newWorkspace")}</span>
        </button>
      </header>

      <div className="workspaces-view-scroll">
        {isAll ? (
          // ================= 全部工作区视图 =================
          <div className="workspaces-all-container">
            <div className="surface workspaces-hero-card">
              <div className="icon-tile workspaces-hero-icon-all">
                <IconHome />
              </div>
              <div className="workspaces-hero-content">
                <h2>{t("workspaces.allWorkspaces")}</h2>
                <p className="workspaces-hero-desc">
                  {t("workspaces.allDescription", {
                    workspaces: workspaces.length,
                    agents: primaryAgents.length,
                  })}
                </p>
                <div className="workspaces-hero-actions">
                  <button
                    type="button"
                    className="btn primary"
                    onClick={onOpenCreateModal}
                  >
                    <IconPlus />
                    <span>{t("workspaces.newWorkspace")}</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("agents")}
                  >
                    <IconAgent />
                    <span>{t("workspaces.viewAllAgents")}</span>
                  </button>
                </div>
              </div>
            </div>

            <div className="workspaces-section">
              <div className="workspaces-section-header">
                <h3>{t("workspaces.allListTitle", { count: workspaces.length })}</h3>
              </div>

              <div className="workspaces-grid">
                {workspaces.map((w) => {
                  const wAgents = primaryAgents.filter(
                    (a) =>
                      normalizeWorkspacePath(a.workspace) ===
                      normalizeWorkspacePath(w.path),
                  );
                  const name = w.name || getWorkspaceBasename(w.path);
                  return (
                    <div key={w.path} className="surface workspace-card">
                      <div className="workspace-card-head">
                        <div className="workspace-card-icon">
                          <IconFolder />
                        </div>
                        <div className="workspace-card-titles">
                          <h4 className="truncate workspace-card-name" title={name}>
                            {name}
                          </h4>
                          <span className="truncate caption workspace-card-path" title={w.path}>
                            {w.path}
                          </span>
                        </div>
                        <span className="workspace-card-badge">
                          {t("workspaces.agentCount", { count: wAgents.length })}
                        </span>
                      </div>

                      <div className="workspace-card-body">
                        {wAgents.length > 0 ? (
                          <div className="workspace-card-agents">
                            {wAgents.slice(0, 4).map((a) => (
                              <span
                                key={a.name}
                                className="workspace-card-agent-tag"
                                onClick={() => onSelectAgent(a.name)}
                                title={t("workspaces.viewAgent", { name: a.name })}
                              >
                                {a.name}
                              </span>
                            ))}
                            {wAgents.length > 4 && (
                              <span className="caption workspace-card-agent-more">
                                +{wAgents.length - 4}
                              </span>
                            )}
                          </div>
                        ) : (
                          <span className="caption workspace-card-empty-agents">
                            {t("workspaces.noAgentsLinked")}
                          </span>
                        )}
                      </div>

                      <div className="workspace-card-actions">
                        <button
                          type="button"
                          className="btn small primary"
                          onClick={() => onSelectWorkspace(w.path)}
                        >
                          <span>{t("workspaces.switchToWorkspace")}</span>
                        </button>
                        <button
                          type="button"
                          className="btn small ghost"
                          title={t("workspaces.newAgentInWorkspace")}
                          onClick={() => onCreateAgent(w.path)}
                        >
                          <IconPlus />
                          <span>{t("nav.newAgent")}</span>
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        ) : (
          // ================= 单个工作区视图 =================
          <div className="workspace-detail-container">
            <div className="surface workspaces-hero-card">
              <div className="icon-tile workspaces-hero-icon">
                <IconFolder />
              </div>
              <div className="workspaces-hero-content">
                <div className="workspaces-hero-title-row">
                  <h2>{displayName}</h2>
                  {currentWorkspace?.custom && (
                    <span className="workspace-tag-custom">{t("workspaces.customWorkspace")}</span>
                  )}
                </div>
                <div className="workspaces-hero-path" title={activeWorkspace}>
                  {activeWorkspace}
                </div>
                <div className="workspaces-hero-stats">
                  <span className="stat-pill">
                    {t("workspaces.linkedAgents", { count: workspaceAgents.length })}
                  </span>
                  <span className="stat-pill">
                    {t("workspaces.activeSessions", { count: totalSessionsCount })}
                  </span>
                </div>
                <div className="workspaces-hero-actions">
                  <button
                    type="button"
                    className="btn primary"
                    onClick={() => onCreateAgent(activeWorkspace)}
                  >
                    <IconPlus />
                    <span>{t("workspaces.newAgentInWorkspace")}</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("agents")}
                  >
                    <IconAgent />
                    <span>{t("workspaces.viewLinkedAgents")}</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("files")}
                  >
                    <IconFolder />
                    <span>{t("workspaces.browseFiles")}</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("git")}
                  >
                    <IconGit />
                    <span>{t("workspaces.viewGit")}</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onSelectWorkspace("all")}
                  >
                    <IconHome />
                    <span>{t("workspaces.backToAll")}</span>
                  </button>
                </div>
              </div>
            </div>

            {/* 工作区关联 Agent */}
            <div className="workspaces-section">
              <div className="workspaces-section-header">
                <h3>{t("workspaces.agentsInWorkspace", { count: workspaceAgents.length })}</h3>
                <span className="spacer" />
                <button
                  type="button"
                  className="btn small ghost"
                  onClick={() => onCreateAgent(activeWorkspace)}
                >
                  <IconPlus />
                  <span>{t("workspaces.addAgent")}</span>
                </button>
              </div>

              {workspaceAgents.length === 0 ? (
                <div className="empty-state workspace-empty-card">
                  <p>{t("workspaces.emptyAgents")}</p>
                  <button
                    type="button"
                    className="btn primary"
                    onClick={() => onCreateAgent(activeWorkspace)}
                  >
                    <IconPlus />
                    <span>{t("workspaces.createFirstAgent")}</span>
                  </button>
                </div>
              ) : (
                <div className="workspaces-agents-grid">
                  {workspaceAgents.map((agent) => (
                    <div key={agent.name} className="surface workspace-agent-card">
                      <div className="workspace-agent-header">
                        <AgentAvatar name={agent.name} subagent={agent.subagent} />
                        <div className="workspace-agent-info">
                          <h4 className="truncate workspace-agent-name">{agent.name}</h4>
                          <span className="truncate workspace-agent-model">
                            {agent.model ?? t("workspace.defaultModel")}
                          </span>
                        </div>
                      </div>
                      <p className="workspace-agent-desc">
                        {agent.description || t("workspaces.noDescription")}
                      </p>
                      <div className="workspace-agent-actions">
                        <button
                          type="button"
                          className="btn small primary"
                          onClick={() => onOpenChat(agent.name, null)}
                        >
                          {t("workspaces.startSession")}
                        </button>
                        <button
                          type="button"
                          className="btn small ghost"
                          onClick={() => onSelectAgent(agent.name)}
                        >
                          {t("workspaces.overview")}
                        </button>
                        <button
                          type="button"
                          className="btn small ghost"
                          onClick={() => onConfigureAgent(agent.name)}
                        >
                          {t("workspaces.configure")}
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>

            {/* 快速切换到其他工作区 */}
            {workspaces.length > 1 && (
              <div className="workspaces-section other-workspaces">
                <div className="workspaces-section-header">
                  <h3>{t("workspaces.quickSwitch")}</h3>
                </div>
                <div className="other-workspaces-pills">
                  {workspaces
                    .filter((w) => w.path !== activeWorkspace)
                    .map((w) => {
                      const name = w.name || getWorkspaceBasename(w.path);
                      return (
                        <button
                          key={w.path}
                          type="button"
                          className="other-workspace-pill"
                          onClick={() => onSelectWorkspace(w.path)}
                          title={w.path}
                        >
                          <IconFolder />
                          <span className="pill-name">{name}</span>
                          <span className="pill-badge">{w.agentCount}</span>
                        </button>
                      );
                    })}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
      </div>
      <WorkspacesRail controller={rail} workspaces={workspaces} activeWorkspace={activeWorkspace}
        agents={agents} sessionsByAgent={sessionsByAgent} onSelectWorkspace={onSelectWorkspace}
        onOpenCreateModal={onOpenCreateModal} onCreateAgent={onCreateAgent}
        onNavigateSection={onNavigateSection} />
    </div>
  );
}
