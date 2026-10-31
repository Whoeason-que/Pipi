import { useState } from "react";
import { IconArchive, IconChevronRight, IconPlus } from "../icons";
import ActionMenu from "../components/ActionMenu";
import AgentAvatar from "../components/AgentAvatar";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { useI18n } from "../i18n";
import type { AgentSection } from "./navigation";
import {
  SESSION_STATUS_LABEL,
  type SessionRunStatus,
} from "./session-status";

export interface AgentSidebarProps {
  section: AgentSection;
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  selected: string | null;
  chatOpen: boolean;
  viewSessionId: string | null;
  archivedSessions: Record<string, SessionSummaryView[]>;
  archivedSessionsExpanded: Record<string, boolean>;
  archivedOpen: boolean;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  sessionStatus: (agentName: string, sessionId: string | null) => SessionRunStatus | undefined;
  onSelectAgent: (name: string) => void;
  onConfigure: (name: string) => void;
  onStartNewSession: (name: string) => void;
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
            aria-label={`${expanded ? "收起" : "展开"} ${agent.name} 的会话`}
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
            <span>{agent.name}</span>
          </button>
          <span className="agent-row-actions">
            <button
              type="button"
              className="icon-btn"
              aria-label={`为 ${agent.name} 新建会话`}
              title="新建会话"
              onClick={() => props.onStartNewSession(agent.name)}
            >
              <IconPlus />
            </button>
            <ActionMenu
              label={`${agent.name} 的更多操作`}
              actions={[
                {
                  label: "Agent 概览",
                  run: () => props.onSelectAgent(agent.name),
                },
                {
                  label: "配置与测试",
                  run: () => props.onConfigure(agent.name),
                },
                {
                  label: "归档 Agent",
                  run: () => props.onArchiveAgent(agent.name),
                },
                {
                  label: "彻底删除 Agent",
                  danger: true,
                  run: () => props.onDeleteAgent(agent.name, false),
                },
              ]}
            />
          </span>
        </div>
        {expanded && (
          <div className="sessions" id={`agent-sessions-${agent.name}`}>
            {sessions.map((session) => {
              const status = props.sessionStatus(agent.name, session.id);
              const active =
                props.chatOpen &&
                props.selected === agent.name &&
                props.viewSessionId === session.id;
              return (
                <div
                  key={session.id}
                  className={`session${status ? ` status-${status}` : ""}${active ? " active" : ""}`}
                >
                  <button
                    type="button"
                    className="session-link"
                    title={
                      status
                        ? `${session.title} · ${SESSION_STATUS_LABEL[status]}`
                        : session.title
                    }
                    onClick={() => props.onOpenSession(agent.name, session.id)}
                    aria-current={active ? "page" : undefined}
                  >
                    <span className="truncate session-title">
                      {session.title || "未命名会话"}
                    </span>
                  </button>
                  <ActionMenu
                    label={`会话 ${session.title} 的更多操作`}
                    actions={[
                      {
                        label: "归档会话",
                        run: () => props.onArchiveSession(agent.name, session.id),
                      },
                      {
                        label: "彻底删除会话",
                        danger: true,
                        run: () => props.onDeleteSession(agent.name, session.id),
                      },
                    ]}
                  />
                </div>
              );
            })}
            {!sessions.length && <div className="caption sidebar-note">尚无会话</div>}
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
              <span className="archive-toggle-label">已归档</span>
            </button>
            {props.archivedSessionsExpanded[agent.name] &&
              (props.archivedSessions[agent.name] ?? []).map((session) => (
                <div className="session archived-session" key={session.id}>
                  <span className="truncate session-title" title={session.title}>
                    {session.title}
                  </span>
                  <ActionMenu
                    label={`归档会话 ${session.title} 的操作`}
                    actions={[
                      {
                        label: "恢复会话",
                        run: () =>
                          props.onRestoreSession(agent.name, session.id),
                      },
                      {
                        label: "彻底删除会话",
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
    <nav className="agents agent-sidebar" aria-label={`${group} 与会话`}>
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
            已归档的 Agent{" "}
            <span className="archive-toggle-meta">
              {props.archivedAgents.length}
            </span>
          </button>
          {props.archivedOpen &&
            props.archivedAgents.map((agent) => (
              <div className="agent-row archived-agent" key={agent.name}>
                <span className="truncate session-title">{agent.name}</span>
                <ActionMenu
                  label={`已归档 Agent ${agent.name} 的操作`}
                  actions={[
                    {
                      label: "恢复 Agent",
                      run: () => props.onRestoreAgent(agent.name),
                    },
                    {
                      label: "彻底删除 Agent",
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
