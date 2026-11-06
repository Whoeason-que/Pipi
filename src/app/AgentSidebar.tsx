import { useState } from "react";
import { IconArchive, IconChevronRight, IconGitBranch, IconPlus } from "../icons";
import ActionMenu from "../components/ActionMenu";
import AgentAvatar from "../components/AgentAvatar";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { useI18n } from "../i18n";
import {
  buildSessionTree,
  type SessionTreeNode,
} from "../chat/session-tree";
import type { AgentSection } from "./navigation";
import {
  sessionStatusLabel,
  type SessionRunStatus,
} from "./session-status";

export interface AgentSidebarProps {
  section: AgentSection;
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  selected: string | null;
  chatOpen: boolean;
  /** 当前打开的会话 id（高亮左栏仓库树里的那条分支）。 */
  viewSessionId: string | null;
  archivedSessions: Record<string, SessionSummaryView[]>;
  archivedSessionsExpanded: Record<string, boolean>;
  archivedOpen: boolean;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  sessionStatus: (agentName: string, sessionId: string | null) => SessionRunStatus | undefined;
  sessionUnread: (agentName: string, sessionId: string | null) => boolean;
  onSelectAgent: (name: string) => void;
  onConfigure: (name: string) => void;
  onStartNewSession: (name: string) => void;
  /** 打开该 Agent 的会话仓库（最近活跃的分支）。 */
  onOpenRepo: (name: string) => void;
  /** 切换左栏仓库树里的分支。 */
  onOpenSession: (name: string, id: string) => void;
  onArchiveAgent: (name: string) => void;
  onDeleteAgent: (name: string, archived: boolean) => void;
  onRestoreAgent: (name: string) => void;
  onArchiveSession: (name: string, id: string) => void;
  onDeleteSession: (name: string, id: string) => void;
  onToggleArchivedSessions: (name: string) => void;
  onRestoreSession: (name: string, id: string) => void;
  onDeleteArchivedSession: (name: string, id: string) => void;
  onToggleArchivedOpen: () => void;
}

export default function AgentSidebar(props: AgentSidebarProps) {
  const { t } = useI18n();
  const [collapsedAgents, setCollapsedAgents] = useState<Set<string>>(
    () => new Set(),
  );
  const toggleAgent = (name: string) => {
    setCollapsedAgents((previous) => {
      const next = new Set(previous);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };
  const subagents = props.section === "subagents";
  const group = t(subagents ? "nav.subagents" : "nav.agents");
  const renderAgent = (agent: AgentDefinition) => {
    const sessions = props.sessionsFor(agent);
    const expanded = !collapsedAgents.has(agent.name);
    const statuses = sessions.map((session) =>
      props.sessionStatus(agent.name, session.id),
    );
    // 仓库行只留一个状态点：运行 > 重试 > 出错 > 完成未读（与旧会话标题色同口径）。
    const repoStatus: SessionRunStatus | undefined = statuses.includes("running")
      ? "running"
      : statuses.includes("retrying")
        ? "retrying"
        : statuses.includes("error")
          ? "error"
          : sessions.some((session) => props.sessionUnread(agent.name, session.id))
            ? "completed"
            : undefined;
    const chatOnAgent = props.chatOpen && props.selected === agent.name;
    const currentSessionId = chatOnAgent ? props.viewSessionId : null;
    // 当前分支在树里时高亮分支本身；草稿/未落分支的会话才高亮仓库行。
    const repoActive =
      chatOnAgent &&
      !sessions.some((session) => session.id === currentSessionId);
    const tree = buildSessionTree(sessions);
    const renderBranch = (node: SessionTreeNode, depth: number) => {
      const session = node.session;
      const status = props.sessionStatus(agent.name, session.id);
      const unread = props.sessionUnread(agent.name, session.id);
      const statusClass =
        status && (status !== "completed" || unread)
          ? ` status-${status}`
          : "";
      const current = session.id === currentSessionId;
      const title = session.title || t("workspace.untitled");
      return (
        <div
          key={session.id}
          className="repo-branch"
          style={{ paddingLeft: depth * 12 }}
        >
          <div
            className={`session branch-row${statusClass}${current ? " active" : ""}`}
          >
            <button
              type="button"
              className="session-link"
              title={
                status
                  ? `${title} · ${sessionStatusLabel(status, unread, t)}`
                  : title
              }
              onClick={() => props.onOpenSession(agent.name, session.id)}
              aria-current={current ? "page" : undefined}
            >
              {(depth > 0 || node.missingParentId) && (
                <span className="branch-fork-mark" aria-hidden="true">
                  ↳
                </span>
              )}
              <span className="truncate session-title fade-end">{title}</span>
            </button>
            <ActionMenu
              label={t("app.sessionMenu", { title })}
              actions={[
                {
                  label: t("app.archiveSession"),
                  run: () => props.onArchiveSession(agent.name, session.id),
                },
                {
                  label: t("app.deleteSession"),
                  danger: true,
                  run: () => props.onDeleteSession(agent.name, session.id),
                },
              ]}
            />
          </div>
          {node.children.map((child) => renderBranch(child, depth + 1))}
        </div>
      );
    };
    return (
      <div className="agent" key={agent.name}>
        <div
          className={`agent-row${props.selected === agent.name && !props.chatOpen ? " selected" : ""}`}
        >
          <button
            type="button"
            className="agent-marker"
            aria-expanded={expanded}
            aria-controls={`agent-sessions-${agent.name}`}
            aria-label={t(
              expanded ? "app.collapseAgentSessions" : "app.expandAgentSessions",
              { name: agent.name },
            )}
            onClick={() => toggleAgent(agent.name)}
          >
            <AgentAvatar
              name={agent.name}
              subagent={agent.subagent}
              className="agent-row-avatar"
            />
            <span className="agent-row-chevron" aria-hidden="true">
              <IconChevronRight />
            </span>
          </button>
          <button
            type="button"
            className="agent-name"
            title={agent.name}
            aria-current={
              props.selected === agent.name && !props.chatOpen
                ? "page"
                : undefined
            }
            onClick={() => props.onSelectAgent(agent.name)}
          >
            <span className="fade-end">{agent.name}</span>
          </button>
          <span className="agent-row-actions">
            <button
              type="button"
              className="icon-btn"
              aria-label={t("app.newSessionFor", { name: agent.name })}
              title={t("nav.newSession")}
              onClick={() => props.onStartNewSession(agent.name)}
            >
              <IconPlus />
            </button>
            <ActionMenu
              label={t("app.agentMenu", { name: agent.name })}
              actions={[
                {
                  label: t("app.agentOverview"),
                  run: () => props.onSelectAgent(agent.name),
                },
                {
                  label: t("workspace.configure"),
                  run: () => props.onConfigure(agent.name),
                },
                {
                  label: t("app.archiveAgent"),
                  run: () => props.onArchiveAgent(agent.name),
                },
                {
                  label: t("app.deleteAgent"),
                  danger: true,
                  run: () => props.onDeleteAgent(agent.name, false),
                },
              ]}
            />
          </span>
        </div>
        {expanded && (
          <div className="sessions" id={`agent-sessions-${agent.name}`}>
            <div className={`session repo-row${repoActive ? " active" : ""}`}>
              <button
                type="button"
                className="session-link"
                title={t("app.openSessionRepo", { name: agent.name })}
                onClick={() => props.onOpenRepo(agent.name)}
                aria-current={repoActive ? "page" : undefined}
              >
                <span className="repo-icon" aria-hidden="true">
                  <IconGitBranch />
                </span>
                <span className="truncate session-title fade-end">sessions.git</span>
                <span
                  className="repo-meta mono"
                  title={t("app.repoBranchCount", { count: sessions.length })}
                >
                  {sessions.length}
                </span>
                {repoStatus && (
                  <span
                    className={`repo-dot status-${repoStatus}`}
                    title={sessionStatusLabel(repoStatus, repoStatus === "completed", t)}
                    aria-label={sessionStatusLabel(repoStatus, repoStatus === "completed", t)}
                  />
                )}
              </button>
            </div>
            {tree.map((node) => renderBranch(node, 0))}
            <button
              type="button"
              className="caption archive-toggle"
              aria-expanded={Boolean(
                props.archivedSessionsExpanded[agent.name],
              )}
              onClick={() => props.onToggleArchivedSessions(agent.name)}
            >
              <span className="archive-toggle-marker" aria-hidden="true">
                <IconArchive />
                <span className="archive-toggle-chevron">
                  <IconChevronRight />
                </span>
              </span>
              <span className="archive-toggle-label">{t("app.archived")}</span>
            </button>
            {props.archivedSessionsExpanded[agent.name] &&
              (props.archivedSessions[agent.name] ?? []).map((session) => (
                <div className="session archived-session" key={session.id}>
                  <span className="truncate session-title fade-end" title={session.title}>
                    {session.title}
                  </span>
                  <ActionMenu
                    label={t("app.archivedSessionMenu", { title: session.title })}
                    actions={[
                      {
                        label: t("app.restoreSession"),
                        run: () =>
                          props.onRestoreSession(agent.name, session.id),
                      },
                      {
                        label: t("app.deleteSession"),
                        danger: true,
                        run: () =>
                          props.onDeleteArchivedSession(agent.name, session.id),
                      },
                    ]}
                  />
                </div>
              ))}
          </div>
        )}
      </div>
    );
  };
  return (
    <nav className="agents agent-sidebar" aria-label={t("app.groupAndSessions", { group })}>
      <section className="agent-sidebar-group">
        {props.agents.map(renderAgent)}
        {!props.agents.length && (
          <div className="caption sidebar-note">
            {t(
              subagents
                ? "workspace.subagentsEmpty"
                : "workspace.createFirst",
            )}
          </div>
        )}
      </section>
      {props.archivedAgents.length > 0 && (
        <section className="agent-sidebar-group">
          <button
            type="button"
            className="caption archive-toggle"
            aria-expanded={props.archivedOpen}
            onClick={props.onToggleArchivedOpen}
          >
            <IconChevronRight expanded={props.archivedOpen} />
            {t("nav.archivedAgents")}{" "}
            <span className="archive-toggle-meta">
              {props.archivedAgents.length}
            </span>
          </button>
          {props.archivedOpen &&
            props.archivedAgents.map((agent) => (
              <div className="agent-row archived-agent" key={agent.name}>
                <span className="truncate session-title fade-end">{agent.name}</span>
                <ActionMenu
                  label={t("app.archivedAgentMenu", { name: agent.name })}
                  actions={[
                    {
                      label: t("app.restoreAgent"),
                      run: () => props.onRestoreAgent(agent.name),
                    },
                    {
                      label: t("app.deleteAgent"),
                      danger: true,
                      run: () => props.onDeleteAgent(agent.name, true),
                    },
                  ]}
                />
              </div>
            ))}
        </section>
      )}
    </nav>
  );
}
