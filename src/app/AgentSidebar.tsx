import { useState } from "react";
import { IconChevronRight, IconEye, IconPlus } from "../icons";
import ActionMenu from "../components/ActionMenu";
import AgentAvatar from "../components/AgentAvatar";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { useI18n } from "../i18n";
import {
  buildSessionGraph,
  buildSessionTree,
  visibleSessions,
  type SessionGraphRow,
} from "../chat/session-tree";
import SessionGraph from "./SessionGraph";
import type { AgentSection } from "./navigation";
import {
  sessionStatusLabel,
  type SessionRunStatus,
} from "./session-status";

/** 会话行尾的紧凑时长（1m/2h/3d/5w）；超过两个月不显示，绝对时间放 title。 */
function compactAge(timestamp: number, now: number): string {
  const minutes = Math.trunc((now - timestamp) / 60_000);
  if (minutes < 1) return "1m";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.trunc(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.trunc(hours / 24);
  if (days < 7) return `${days}d`;
  const weeks = Math.trunc(days / 7);
  if (weeks < 9) return `${weeks}w`;
  return "";
}

export interface AgentSidebarProps {
  section: AgentSection;
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  selected: string | null;
  chatOpen: boolean;
  /** 当前打开的会话 id（高亮左栏仓库树里的那条分支）。 */
  viewSessionId: string | null;
  archivedOpen: boolean;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  archivedSessionsByAgent: Record<string, SessionSummaryView[]>;
  showArchived: boolean;
  onToggleArchivedSessions: () => void;
  sessionStatus: (agentName: string, sessionId: string | null) => SessionRunStatus | undefined;
  sessionUnread: (agentName: string, sessionId: string | null) => boolean;
  onSelectAgent: (name: string) => void;
  onConfigure: (name: string) => void;
  onStartNewSession: (name: string) => void;
  /** 切换左栏仓库树里的分支。 */
  onOpenSession: (name: string, id: string) => void;
  onArchiveAgent: (name: string) => void;
  onDeleteAgent: (name: string, archived: boolean) => void;
  onRestoreAgent: (name: string) => void;
  onArchiveSession: (name: string, id: string) => void;
  onRestoreSession: (name: string, id: string) => void;
  onDeleteSession: (name: string, id: string, archived: boolean) => void;
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
    const activeSessions = props.sessionsFor(agent);
    const archivedSessions = props.archivedSessionsByAgent[agent.name] ?? [];
    const archivedIds = new Set(archivedSessions.map((session) => session.id));
    const sessions = visibleSessions(
      [...activeSessions, ...archivedSessions].sort((a, b) => b.lastActive - a.lastActive),
      archivedIds,
      props.showArchived,
    );
    const expanded = !collapsedAgents.has(agent.name);
    const statuses = sessions.map((session) =>
      props.sessionStatus(agent.name, session.id),
    );
    // 头部聚合状态点：运行 > 重试 > 出错 > 完成未读（与旧会话标题色同口径）。
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
    // 当前分支在树里时高亮分支本身；草稿/未落分支的会话高亮 Agent 头部行。
    const repoActive =
      chatOnAgent &&
      !activeSessions.some((session) => session.id === currentSessionId);
    const graph = buildSessionGraph(buildSessionTree(sessions));
    const now = Date.now();
    const renderBranch = (row: SessionGraphRow) => {
      const { node } = row;
      const session = node.session;
      const archived = archivedIds.has(session.id);
      const status = props.sessionStatus(agent.name, session.id);
      const unread = props.sessionUnread(agent.name, session.id);
      const statusClass =
        status && (status !== "completed" || unread)
          ? ` status-${status}`
          : "";
      const current = session.id === currentSessionId;
      const title = session.title || t("workspace.untitled");
      const age = compactAge(session.lastActive, now);
      return (
        <div key={session.id} className="repo-branch">
          <div
            className={`session branch-row${statusClass}${current ? " active" : ""}${archived ? " archived-session" : ""}`}
          >
            <button
              type="button"
              className="session-link"
              disabled={archived}
              title={
                archived ? t("app.restoreSessionToOpen") : status
                  ? `${title} · ${sessionStatusLabel(status, unread, t)}`
                  : title
              }
              onClick={() => props.onOpenSession(agent.name, session.id)}
              aria-current={current ? "page" : undefined}
            >
              <SessionGraph row={row} current={current} />
              <span className="truncate session-title fade-end">{title}</span>
              {age && (
                <span
                  className="session-when mono"
                  title={new Date(session.lastActive).toLocaleString()}
                >
                  {age}
                </span>
              )}
            </button>
            <ActionMenu
              label={t(archived ? "app.archivedSessionMenu" : "app.sessionMenu", { title })}
              actions={[
                archived
                  ? { label: t("app.restoreSession"), run: () => props.onRestoreSession(agent.name, session.id) }
                  : { label: t("app.archiveSession"), run: () => props.onArchiveSession(agent.name, session.id) },
                {
                  label: t("app.deleteSession"),
                  danger: true,
                  run: () => props.onDeleteSession(agent.name, session.id, archived),
                },
              ]}
            />
          </div>
        </div>
      );
    };
    return (
      <div className="agent" key={agent.name}>
        <div
          className={`agent-row${props.selected === agent.name && !props.chatOpen ? " selected" : ""}${repoActive ? " draft-active" : ""}`}
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
              (props.selected === agent.name && !props.chatOpen) || repoActive
                ? "page"
                : undefined
            }
            onClick={() => props.onSelectAgent(agent.name)}
          >
            <span className="fade-end">{agent.name}</span>
          </button>
          <span className="agent-row-meta">
            {sessions.length > 0 && (
              <span
                className="repo-meta mono"
                title={t("app.repoBranchCount", { count: sessions.length })}
              >
                {sessions.length}
              </span>
            )}
            {repoStatus && (
              <span
                className={`repo-dot status-${repoStatus}`}
                title={sessionStatusLabel(repoStatus, repoStatus === "completed", t)}
                aria-label={sessionStatusLabel(repoStatus, repoStatus === "completed", t)}
              />
            )}
          </span>
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
            {graph.map(renderBranch)}
          </div>
        )}
      </div>
    );
  };
  return (
    <nav className="agents agent-sidebar" aria-label={t("app.groupAndSessions", { group })}>
      <div className="caption sb-sec agent-sidebar-heading">
        <span>{group}</span>
        <span className="spacer" />
        <span className="caption sb-count">{props.agents.length}</span>
        <button
          type="button"
          className={`icon-btn${props.showArchived ? " active" : ""}`}
          title={t(props.showArchived ? "app.hideArchivedSessions" : "app.showArchivedSessions")}
          aria-label={t(props.showArchived ? "app.hideArchivedSessions" : "app.showArchivedSessions")}
          aria-pressed={props.showArchived}
          onClick={props.onToggleArchivedSessions}
        >
          <IconEye />
        </button>
      </div>
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
