import { useState } from "react";
import { IconArchive, IconChevronRight, IconPlus } from "../icons";
import ActionMenu from "../components/ActionMenu";
import AgentAvatar from "../components/AgentAvatar";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { useI18n } from "../i18n";
import type { AgentSection } from "./navigation";

export interface AgentSidebarProps {
  section: AgentSection;
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  selected: string | null;
  chatOpen: boolean;
  viewSessionId: string | null;
  expandedAgents: Record<string, boolean>;
  archivedSessions: Record<string, SessionSummaryView[]>;
  archivedSessionsExpanded: Record<string, boolean>;
  archivedOpen: boolean;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  agentHasRunning: (name: string) => boolean;
  isConversationRunning: (name: string, id: string | null) => boolean;
  onCreateAgent: () => void;
  onSelectAgent: (name: string) => void;
  onConfigure: (name: string) => void;
  onStartNewSession: (name: string) => void;
  onOpenSession: (name: string, id: string) => void;
  onToggleAgentExpanded: (name: string, expanded: boolean) => void;
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
  const [collapsed, setCollapsed] = useState(false);
  const subagents = props.section === "subagents";
  const group = t(subagents ? "nav.subagents" : "nav.agents");
  const renderAgent = (agent: AgentDefinition) => {
    const sessions = props.sessionsFor(agent);
    const expanded =
      props.expandedAgents[agent.name] ?? props.selected === agent.name;
    return (
      <div className="agent" key={agent.name}>
        <div
          className={`agent-row${props.selected === agent.name && !props.chatOpen ? " selected" : ""}`}
        >
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
            <AgentAvatar
              name={agent.name}
              subagent={agent.subagent}
              className="sidebar-avatar"
            />
            <span>{agent.name}</span>
          </button>
          {props.agentHasRunning(agent.name) && (
            <span className="activity-dot running" aria-label="正在运行" />
          )}
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
          <button
            type="button"
            className="agent-expand-btn"
            aria-expanded={expanded}
            aria-controls={`agent-sessions-${agent.name}`}
            aria-label={`${expanded ? "收起" : "展开"} ${agent.name} 的会话`}
            onClick={() => props.onToggleAgentExpanded(agent.name, !expanded)}
          >
            <IconChevronRight expanded={expanded} />
          </button>
        </div>
        {expanded && (
          <div className="sessions" id={`agent-sessions-${agent.name}`}>
            {sessions.map((session) => (
              <div
                key={session.id}
                className={`session${props.chatOpen && props.selected === agent.name && props.viewSessionId === session.id ? " active" : ""}`}
              >
                <button
                  type="button"
                  className="session-link"
                  title={session.title}
                  onClick={() => props.onOpenSession(agent.name, session.id)}
                  aria-current={
                    props.chatOpen &&
                    props.selected === agent.name &&
                    props.viewSessionId === session.id
                      ? "page"
                      : undefined
                  }
                >
                  <span className="session-marker">
                    <span
                      className={`activity-dot${props.isConversationRunning(agent.name, session.id) ? " running" : ""}`}
                    />
                  </span>
                  <span className="session-title">
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
            ))}
            {!sessions.length && <div className="sidebar-note">尚无会话</div>}
            <button
              type="button"
              className="archive-toggle"
              aria-expanded={Boolean(
                props.archivedSessionsExpanded[agent.name],
              )}
              onClick={() => props.onToggleArchivedSessions(agent.name)}
            >
              <span className="archive-toggle-label">
                <span className="session-marker">
                  <IconArchive />
                </span>
                已归档
              </span>
              <IconChevronRight
                expanded={Boolean(props.archivedSessionsExpanded[agent.name])}
              />
            </button>
            {props.archivedSessionsExpanded[agent.name] &&
              (props.archivedSessions[agent.name] ?? []).map((session) => (
                <div className="session archived-session" key={session.id}>
                  <span className="session-marker">
                    <span className="activity-dot" />
                  </span>
                  <span className="session-title" title={session.title}>
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
        <div className="sb-sec agent-group-heading">
          <button
            type="button"
            aria-expanded={!collapsed}
            onClick={() => setCollapsed((previous) => !previous)}
          >
            <IconChevronRight expanded={!collapsed} />
            <span>{group}</span>
            <span className="sb-count">{props.agents.length}</span>
          </button>
          {!subagents && (
            <button
              type="button"
              className="icon-btn"
              title={t("nav.newAgent")}
              aria-label={t("nav.newAgent")}
              onClick={props.onCreateAgent}
            >
              <IconPlus />
            </button>
          )}
        </div>
        {!collapsed && (
          <>
            {props.agents.map(renderAgent)}
            {!props.agents.length && (
              <div className="sidebar-note">
                {t(
                  subagents
                    ? "workspace.subagentsEmpty"
                    : "workspace.createFirst",
                )}
              </div>
            )}
          </>
        )}
      </section>
      {props.archivedAgents.length > 0 && (
        <section className="agent-sidebar-group">
          <button
            type="button"
            className="archive-toggle"
            aria-expanded={props.archivedOpen}
            onClick={props.onToggleArchivedOpen}
          >
            已归档的 Agent{" "}
            <span className="archive-toggle-meta">
              {props.archivedAgents.length}
              <IconChevronRight expanded={props.archivedOpen} />
            </span>
          </button>
          {props.archivedOpen &&
            props.archivedAgents.map((agent) => (
              <div className="agent-row archived-agent" key={agent.name}>
                <span className="session-title">{agent.name}</span>
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
