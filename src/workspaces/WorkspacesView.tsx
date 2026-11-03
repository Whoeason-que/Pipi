import { NavigationToggle } from "../app/AppNavigation";
import type { CSSProperties } from "react";
import { useRightRail } from "../app/right-rail";
import AgentAvatar from "../components/AgentAvatar";
import {
  IconAgent,
  IconBack,
  IconFolder,
  IconGit,
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
  const agentCountLabel = (count: number) =>
    t(count === 1 ? "workspaces.agentCountOne" : "workspaces.agentCount", {
      count,
    });
  const sessionCountLabel = (count: number) =>
    t(count === 1 ? "common.sessionCountOne" : "common.sessionCount", {
      count,
    });

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
      </header>

      <div className="page-scroll">
        {isAll ? (
          // ================= 全部工作区视图 =================
          <div className="workspaces-all-container">
            <header className="page-heading workspaces-all-heading">
              <div className="page-heading-intro">
                <h1>
                  {t("workspaces.allWorkspaces")}
                  <span className="section-count">{workspaces.length}</span>
                </h1>
                <p>{t("workspaces.allDescription")}</p>
              </div>
              <div className="page-heading-actions">
                <button
                  type="button"
                  className="btn primary"
                  onClick={onOpenCreateModal}
                >
                  <IconPlus />
                  <span>{t("workspaces.newWorkspace")}</span>
                </button>
              </div>
            </header>

            {workspaces.length === 0 ? (
              <div className="empty-card">
                <span className="empty-icon">
                  <IconFolder />
                </span>
                <h3>{t("workspaces.emptyWorkspaces")}</h3>
                <p>{t("workspaces.emptyWorkspacesHint")}</p>
                <button
                  type="button"
                  className="btn primary"
                  onClick={onOpenCreateModal}
                >
                  <IconPlus />
                  <span>{t("workspaces.newWorkspace")}</span>
                </button>
              </div>
            ) : (
              <div className="workspaces-grid">
                {workspaces.map((w) => {
                  const wAgents = primaryAgents.filter(
                    (a) =>
                      normalizeWorkspacePath(a.workspace) ===
                      normalizeWorkspacePath(w.path),
                  );
                  const name = w.name || getWorkspaceBasename(w.path);
                  return (
                    <article key={w.path} className="surface workspace-card">
                      <div className="workspace-card-head">
                        <div className="workspace-card-icon">
                          <IconFolder />
                        </div>
                        <div className="workspace-card-titles">
                          <h2
                            className="truncate workspace-card-name"
                            title={name}
                          >
                            {name}
                          </h2>
                          <span
                            className="truncate caption workspace-card-path"
                            title={w.path}
                          >
                            {w.path}
                          </span>
                        </div>
                        <span className="workspace-card-badge">
                          {agentCountLabel(wAgents.length)}
                        </span>
                      </div>

                      <div className="workspace-card-body">
                        {wAgents.length > 0 ? (
                          <div className="workspace-card-agents">
                            {wAgents.slice(0, 4).map((a) => (
                              <button
                                type="button"
                                key={a.name}
                                className="workspace-agent-row"
                                onClick={() => onSelectAgent(a.name)}
                                title={t("workspaces.viewAgent", { name: a.name })}
                              >
                                <AgentAvatar
                                  name={a.name}
                                  subagent={a.subagent}
                                  className="workspace-agent-row-avatar"
                                />
                                <span className="workspace-agent-row-name">
                                  {a.name}
                                </span>
                                <span className="workspace-agent-row-meta">
                                  {sessionCountLabel(
                                    sessionsByAgent[a.name]?.length ?? 0,
                                  )}
                                </span>
                              </button>
                            ))}
                            {wAgents.length > 4 && (
                              <button
                                type="button"
                                className="workspace-card-more"
                                onClick={() => onSelectWorkspace(w.path)}
                              >
                                +{wAgents.length - 4}
                              </button>
                            )}
                          </div>
                        ) : (
                          <span className="workspace-card-empty-agents">
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
                          <span>{t("workspaces.openWorkspace")}</span>
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
                    </article>
                  );
                })}
              </div>
            )}
          </div>
        ) : (
          // ================= 单个工作区视图 =================
          <div className="workspace-detail-container">
            <header className="page-heading workspaces-detail-heading">
              <div className="page-heading-intro">
                <button
                  type="button"
                  className="workspaces-back"
                  onClick={() => onSelectWorkspace("all")}
                >
                  <IconBack />
                  <span>{t("workspaces.backToAll")}</span>
                </button>
                <div className="workspaces-detail-title">
                  <span className="icon-tile workspaces-hero-icon">
                    <IconFolder />
                  </span>
                  <div className="workspaces-detail-copy">
                    <div className="workspaces-hero-title-row">
                      <h1>{displayName}</h1>
                      {currentWorkspace?.custom && (
                        <span className="workspace-tag-custom">
                          {t("workspaces.customWorkspace")}
                        </span>
                      )}
                    </div>
                    <div
                      className="workspaces-hero-path"
                      title={activeWorkspace}
                    >
                      {activeWorkspace}
                    </div>
                    <div className="workspaces-detail-meta">
                      <span>{agentCountLabel(workspaceAgents.length)}</span>
                      <span>{sessionCountLabel(totalSessionsCount)}</span>
                    </div>
                  </div>
                </div>
              </div>
              <div className="page-heading-actions">
                <button
                  type="button"
                  className="btn primary"
                  title={t("workspaces.newAgentInWorkspace")}
                  onClick={() => onCreateAgent(activeWorkspace)}
                >
                  <IconPlus />
                  <span>{t("nav.newAgent")}</span>
                </button>
                <button
                  type="button"
                  className="btn ghost"
                  onClick={() => onNavigateSection("agents")}
                >
                  <IconAgent />
                  <span>{t("workspaces.agentsShort")}</span>
                </button>
                <button
                  type="button"
                  className="btn ghost"
                  onClick={() => onNavigateSection("files")}
                >
                  <IconFolder />
                  <span>{t("workspaces.filesShort")}</span>
                </button>
                <button
                  type="button"
                  className="btn ghost"
                  onClick={() => onNavigateSection("git")}
                >
                  <IconGit />
                  <span>{t("workspaces.gitShort")}</span>
                </button>
              </div>
            </header>

            {/* 工作区关联 Agent */}
            <div className="workspaces-section">
              <div className="section-heading section-heading-ruled">
                <h2>
                  {t("workspaces.agentsInWorkspace", {
                    count: workspaceAgents.length,
                  })}
                </h2>
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
                <div className="empty-card">
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
                    <article
                      key={agent.name}
                      className="surface workspace-agent-card"
                    >
                      <div className="workspace-agent-header">
                        <AgentAvatar
                          name={agent.name}
                          subagent={agent.subagent}
                        />
                        <div className="workspace-agent-info">
                          <h3 className="truncate workspace-agent-name">
                            {agent.name}
                          </h3>
                          <span className="truncate workspace-agent-model">
                            {agent.provider?.id ||
                              agent.model ||
                              t("workspace.noModel")}
                          </span>
                        </div>
                        <span className="workspace-agent-sessions">
                          {sessionCountLabel(
                            sessionsByAgent[agent.name]?.length ?? 0,
                          )}
                        </span>
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
                    </article>
                  ))}
                </div>
              )}
            </div>

            {/* 快速切换到其他工作区 */}
            {workspaces.length > 1 && (
              <div className="workspaces-section other-workspaces">
                <div className="section-heading section-heading-ruled">
                  <h2>{t("workspaces.quickSwitch")}</h2>
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
