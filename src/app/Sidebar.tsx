import {
  IconArchive,
  IconGear,
  IconPlus,
  IconRestore,
  IconTrash
} from "../icons";
import type { AgentDefinition, SessionInfoView, SessionSummaryView } from "../types";

export interface SidebarProps {
  /** 当前分区只可能是 agents / subs（搜索与技能各自有面板）。 */
  navSection: "agents" | "subs";
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  selected: string | null;
  creating: boolean;
  chatOpen: boolean;
  viewSessionId: string | null;
  activeSession: SessionInfoView | null;
  expandedAgents: Record<string, boolean>;
  archivedSessions: Record<string, SessionSummaryView[]>;
  archivedSessionsExpanded: Record<string, boolean>;
  archivedOpen: boolean;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  agentHasRunning: (agentName: string) => boolean;
  isConversationRunning: (agentName: string, sessionId: string | null) => boolean;
  onCreateAgent: () => void;
  onSelectAgent: (agentName: string) => void;
  onStartNewSession: (agentName: string) => void;
  onOpenSession: (agentName: string, sessionId: string) => void;
  onToggleAgentExpanded: (agentName: string, expanded: boolean) => void;
  onArchiveAgent: (name: string) => void;
  onDeleteAgent: (name: string, archived: boolean) => void;
  onRestoreAgent: (name: string) => void;
  onArchiveSession: (agentName: string, sessionId: string) => void;
  onDeleteSession: (agentName: string, sessionId: string) => void;
  onToggleArchivedSessions: (agentName: string) => void;
  onRestoreSession: (agentName: string, sessionId: string) => void;
  onDeleteArchivedSession: (agentName: string, sessionId: string) => void;
  onToggleArchivedOpen: () => void;
}

/** Agent / Subagent 侧栏：Agent 列表、展开的会话列表与归档区。 */
export function Sidebar({
  navSection,
  agents,
  archivedAgents,
  selected,
  creating,
  chatOpen,
  viewSessionId,
  activeSession,
  expandedAgents,
  archivedSessions,
  archivedSessionsExpanded,
  archivedOpen,
  sessionsFor,
  agentHasRunning,
  isConversationRunning,
  onCreateAgent,
  onSelectAgent,
  onStartNewSession,
  onOpenSession,
  onToggleAgentExpanded,
  onArchiveAgent,
  onDeleteAgent,
  onRestoreAgent,
  onArchiveSession,
  onDeleteSession,
  onToggleArchivedSessions,
  onRestoreSession,
  onDeleteArchivedSession,
  onToggleArchivedOpen,
}: SidebarProps) {
  return (
    <>
      <div className="sb-sec agent-group-heading">
        <span>{navSection === "subs" ? "SUBS" : "AGENTS"}</span>
        <span className="spacer" />
        <span className="sb-count">{agents.length}</span>
        {navSection === "agents" && (
          <button
            type="button"
            className="icon-btn add"
            title="新建 Agent"
            aria-label="新建 Agent"
            onClick={onCreateAgent}
          >
            <IconPlus />
          </button>
        )}
      </div>

      <nav
        className="agents"
        aria-label={navSection === "subs" ? "Subagent 列表" : "Agent 列表"}
      >
        {agents.map((a) => {
          const isActiveAgent = selected === a.name && !creating;
          const sessions = sessionsFor(a);
          const sessionCount = sessions.length;
          const isExpanded = expandedAgents[a.name]
            ?? (isActiveAgent && chatOpen)
            ?? false;

          const isRunning = agentHasRunning(a.name);
          return (
            <div key={a.name} className="agent">
              <div className="agent-row">
                <button
                  type="button"
                  className="agent-name"
                  aria-current={isActiveAgent && !chatOpen ? "true" : undefined}
                  title={a.description || a.name}
                  onClick={() => onSelectAgent(a.name)}
                >
                  {a.name}
                </button>
                {isRunning && (
                  <span className="agent-running" title={`${a.name} 正在运行`}>
                    运行中
                  </span>
                )}
                <span className="agent-actions">
                  <button
                    type="button"
                    className="icon-btn"
                    title="Agent 配置"
                    aria-label={`打开 ${a.name} 的配置`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onSelectAgent(a.name);
                    }}
                  >
                    <IconGear />
                  </button>
                  <button
                    type="button"
                    className="icon-btn"
                    title="新建会话"
                    aria-label={`为 ${a.name} 新建会话`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onStartNewSession(a.name);
                    }}
                  >
                    <IconPlus />
                  </button>
                  <button
                    type="button"
                    className="icon-btn"
                    title={`归档 ${a.name}（移入 .archive/）`}
                    aria-label={`归档 ${a.name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onArchiveAgent(a.name);
                    }}
                  >
                    <IconArchive />
                  </button>
                  <button
                    type="button"
                    className="icon-btn danger"
                    title={`彻底删除 ${a.name}`}
                    aria-label={`彻底删除 ${a.name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onDeleteAgent(a.name, false);
                    }}
                  >
                    <IconTrash />
                  </button>
                </span>
                <span className="agent-count">{sessionCount}</span>
                {sessionCount > 0 ? (
                  <button
                    type="button"
                    className="agent-expand-btn"
                    aria-expanded={isExpanded}
                    title={isExpanded ? "收起会话" : "展开会话"}
                    aria-label={isExpanded ? "收起会话" : "展开会话"}
                    onClick={(event) => {
                      event.stopPropagation();
                      onToggleAgentExpanded(a.name, !isExpanded);
                    }}
                  >
                    <span className={`caret${isExpanded ? " open" : ""}`}>▶</span>
                  </button>
                ) : (
                  <span className="agent-expand-placeholder" />
                )}
              </div>
              {isExpanded && (
                <div className="sessions">
                  {sessions.map((sess) => {
                    const isCurrent = chatOpen && selected === a.name && viewSessionId === sess.id;
                    const isRunning = isConversationRunning(a.name, sess.id);
                    return (
                      <div
                        key={sess.id}
                        className={`session${isCurrent ? " active" : ""}`}
                        role="button"
                        tabIndex={0}
                        aria-current={isCurrent ? "true" : undefined}
                        title={`${sess.title}（${sess.messageCount} 条消息）`}
                        onClick={() => onOpenSession(a.name, sess.id)}
                        onKeyDown={(event) => {
                          if (event.target !== event.currentTarget) return;
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            onOpenSession(a.name, sess.id);
                          }
                        }}
                      >
                        <div className="session-content">
                          <span className="session-title">{sess.title}</span>
                        </div>
                        <div className="session-meta">
                          <span className="session-status">
                            {isRunning ? (
                              <span className="session-running" title="这条会话正在运行">
                                运行中
                              </span>
                            ) : (
                              <span className="session-idle">空闲</span>
                            )}
                          </span>
                          <span className="session-actions">
                            <button
                              type="button"
                              className="icon-btn"
                              title="归档会话"
                              aria-label={`归档会话 ${sess.title}`}
                              onClick={(event) => {
                                event.stopPropagation();
                                onArchiveSession(a.name, sess.id);
                              }}
                            >
                              <IconArchive />
                            </button>
                            <button
                              type="button"
                              className="icon-btn danger"
                              title="彻底删除会话"
                              aria-label={`彻底删除会话 ${sess.title}`}
                              onClick={(event) => {
                                event.stopPropagation();
                                onDeleteSession(a.name, sess.id);
                              }}
                            >
                              <IconTrash />
                            </button>
                          </span>
                        </div>
                      </div>
                    );
                  })}
                  {isActiveAgent && chatOpen && !activeSession && (
                    <div className="session pending">（新会话）</div>
                  )}
                  <div
                    className="session archived-toggle"
                    role="button"
                    tabIndex={0}
                    aria-expanded={Boolean(archivedSessionsExpanded[a.name])}
                    onClick={() => onToggleArchivedSessions(a.name)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        onToggleArchivedSessions(a.name);
                      }
                    }}
                  >
                    <IconArchive />
                    <span className="session-title dim">已归档</span>
                    <span className="session-count">{archivedSessions[a.name]?.length ?? ""}</span>
                    <span className={`caret${archivedSessionsExpanded[a.name] ? " open" : ""}`}>▶</span>
                  </div>
                  {archivedSessionsExpanded[a.name] && (
                    <div className="archived-sessions">
                      {(archivedSessions[a.name] ?? []).length === 0 ? (
                        <div className="session pending">没有归档会话</div>
                      ) : (
                        (archivedSessions[a.name] ?? []).map((sess) => (
                          <div key={sess.id} className="session">
                            <div className="session-content">
                              <span className="session-title dim">{sess.title}</span>
                            </div>
                            <div className="session-meta">
                              <span className="session-status session-archived-status">已归档</span>
                              <span className="session-actions">
                                <button
                                  type="button"
                                  className="icon-btn"
                                  title="恢复会话"
                                  aria-label={`恢复会话 ${sess.title}`}
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    onRestoreSession(a.name, sess.id);
                                  }}
                                >
                                  <IconRestore />
                                </button>
                                <button
                                  type="button"
                                  className="icon-btn danger"
                                  title="彻底删除归档会话"
                                  aria-label={`彻底删除归档会话 ${sess.title}`}
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    onDeleteArchivedSession(a.name, sess.id);
                                  }}
                                >
                                  <IconTrash />
                                </button>
                              </span>
                            </div>
                          </div>
                        ))
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
        {agents.length === 0 && (
          <div className="session pending">
            {navSection === "subs"
              ? "没有 Subagent：由 Agent 的 create_agent 工具创建后归入这里"
              : "还没有 Agent：点上方 + 新建"}
          </div>
        )}
      </nav>

      {archivedAgents.length > 0 && (
        <>
          <div
            className="sb-sec archived-head"
            role="button"
            tabIndex={0}
            aria-expanded={archivedOpen}
            onClick={onToggleArchivedOpen}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onToggleArchivedOpen();
              }
            }}
          >
            <span>已归档</span>
            <span className="spacer" />
            <span className="sb-count">{archivedAgents.length}</span>
            <span className={`caret${archivedOpen ? " open" : ""}`}>▶</span>
          </div>
          {archivedOpen && (
            <div className="archived">
              {archivedAgents.map((agent) => (
                <div key={agent.name} className="agent-row">
                  <span className="agent-name dim">{agent.name}</span>
                  <span className="agent-actions">
                    <button
                      type="button"
                      className="icon-btn"
                      title={`恢复 ${agent.name}`}
                      aria-label={`恢复 ${agent.name}`}
                      onClick={(event) => {
                        event.stopPropagation();
                        onRestoreAgent(agent.name);
                      }}
                    >
                      <IconRestore />
                    </button>
                    <button
                      type="button"
                      className="icon-btn danger"
                      title={`彻底删除 ${agent.name}`}
                      aria-label={`彻底删除 ${agent.name}`}
                      onClick={(event) => {
                        event.stopPropagation();
                        onDeleteAgent(agent.name, true);
                      }}
                    >
                      <IconTrash />
                    </button>
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </>
  );
}
