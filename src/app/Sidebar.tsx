import { useState } from "react";
import { IconPlus } from "../icons";
import ActionMenu from "../components/ActionMenu";
import type { AgentDefinition, SessionSummaryView } from "../types";

export interface SidebarProps {
  agents: AgentDefinition[]; archivedAgents: AgentDefinition[]; selected: string | null;
  chatOpen: boolean; viewSessionId: string | null;
  expandedAgents: Record<string, boolean>;
  archivedSessions: Record<string, SessionSummaryView[]>;
  archivedSessionsExpanded: Record<string, boolean>; archivedOpen: boolean;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  agentHasRunning: (name: string) => boolean;
  isConversationRunning: (name: string, id: string | null) => boolean;
  onCreateAgent: () => void; onSelectAgent: (name: string) => void; onConfigure: (name: string) => void;
  onStartNewSession: (name: string) => void; onOpenSession: (name: string, id: string) => void;
  onToggleAgentExpanded: (name: string, expanded: boolean) => void;
  onArchiveAgent: (name: string) => void; onDeleteAgent: (name: string, archived: boolean) => void;
  onRestoreAgent: (name: string) => void; onArchiveSession: (name: string, id: string) => void;
  onDeleteSession: (name: string, id: string) => void; onToggleArchivedSessions: (name: string) => void;
  onRestoreSession: (name: string, id: string) => void; onDeleteArchivedSession: (name: string, id: string) => void;
  onToggleArchivedOpen: () => void;
}

export function Sidebar(props: SidebarProps) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const renderAgent = (agent: AgentDefinition) => {
    const sessions = props.sessionsFor(agent);
    const expanded = props.expandedAgents[agent.name] ?? (props.selected === agent.name);
    return <div className="agent" key={agent.name}>
      <div className={`agent-row${props.selected === agent.name && !props.chatOpen ? " selected" : ""}`}>
        <button type="button" className="agent-expand-btn" aria-expanded={expanded} aria-label={`${expanded ? "收起" : "展开"} ${agent.name} 的会话`}
          onClick={() => props.onToggleAgentExpanded(agent.name, !expanded)}><span className={`caret${expanded ? " open" : ""}`}>▶</span></button>
        <button type="button" className="agent-name" title={agent.name} aria-current={props.selected === agent.name && !props.chatOpen ? "page" : undefined}
          onClick={() => props.onSelectAgent(agent.name)}>{agent.name}</button>
        {props.agentHasRunning(agent.name) && <span className="activity-dot running" aria-label="正在运行" />}
        <span className="agent-row-actions"><button type="button" className="icon-btn" aria-label={`为 ${agent.name} 新建会话`} title="新建会话"
          onClick={() => props.onStartNewSession(agent.name)}><IconPlus /></button>
          <ActionMenu label={`${agent.name} 的更多操作`} actions={[
            { label: "Agent 概览", run: () => props.onSelectAgent(agent.name) },
            { label: "配置与测试", run: () => props.onConfigure(agent.name) },
            { label: "归档 Agent", run: () => props.onArchiveAgent(agent.name) },
            { label: "彻底删除 Agent", danger: true, run: () => props.onDeleteAgent(agent.name, false) },
          ]} /></span>
      </div>
      {expanded && <div className="sessions">
        {sessions.map(session => <div key={session.id} className={`session${props.chatOpen && props.selected === agent.name && props.viewSessionId === session.id ? " active" : ""}`}>
          <button type="button" className="session-link" title={session.title} onClick={() => props.onOpenSession(agent.name, session.id)}
            aria-current={props.chatOpen && props.selected === agent.name && props.viewSessionId === session.id ? "page" : undefined}>
            <span className={`activity-dot${props.isConversationRunning(agent.name, session.id) ? " running" : ""}`} />
            <span className="session-title">{session.title || "未命名会话"}</span></button>
          <ActionMenu label={`会话 ${session.title} 的更多操作`} actions={[
            { label: "归档会话", run: () => props.onArchiveSession(agent.name, session.id) },
            { label: "彻底删除会话", danger: true, run: () => props.onDeleteSession(agent.name, session.id) },
          ]} />
        </div>)}
        {!sessions.length && <div className="sidebar-note">尚无会话</div>}
        <button type="button" className="archive-toggle" aria-expanded={Boolean(props.archivedSessionsExpanded[agent.name])}
          onClick={() => props.onToggleArchivedSessions(agent.name)}>已归档 <span>{props.archivedSessionsExpanded[agent.name] ? "−" : "+"}</span></button>
        {props.archivedSessionsExpanded[agent.name] && (props.archivedSessions[agent.name] ?? []).map(session => <div className="session archived-session" key={session.id}>
          <span className="session-title" title={session.title}>{session.title}</span><ActionMenu label={`归档会话 ${session.title} 的操作`} actions={[
            { label: "恢复会话", run: () => props.onRestoreSession(agent.name, session.id) },
            { label: "彻底删除会话", danger: true, run: () => props.onDeleteArchivedSession(agent.name, session.id) },
          ]} /></div>)}
      </div>}
    </div>;
  };
  return <nav className="agents tree-sidebar" aria-label="Agent 与会话">
    {([false, true] as const).map(subagent => {
      const group = subagent ? "Subagents" : "Agents";
      const list = props.agents.filter(agent => agent.subagent === subagent);
      return <section key={group} className="agent-tree-group"><div className="sb-sec agent-group-heading">
        <button type="button" aria-expanded={!collapsed[group]} onClick={() => setCollapsed(previous => ({ ...previous, [group]: !previous[group] }))}>
          <span>{group}</span><span className="sb-count">{list.length}</span></button>
        {!subagent && <button type="button" className="icon-btn" title="新建 Agent" aria-label="新建 Agent" onClick={props.onCreateAgent}><IconPlus /></button>}
      </div>{!collapsed[group] && <>{list.map(renderAgent)}{!list.length && <div className="sidebar-note">{subagent ? "组合能力创建的 Agent 会出现在这里" : "创建一个 Agent，开始工作"}</div>}</>}</section>;
    })}
    {props.archivedAgents.length > 0 && <section className="agent-tree-group"><button type="button" className="archive-toggle" aria-expanded={props.archivedOpen}
      onClick={props.onToggleArchivedOpen}>已归档的 Agent <span>{props.archivedAgents.length}</span></button>
      {props.archivedOpen && props.archivedAgents.map(agent => <div className="agent-row archived-agent" key={agent.name}><span className="session-title">{agent.name}</span>
        <ActionMenu label={`已归档 Agent ${agent.name} 的操作`} actions={[
          { label: "恢复 Agent", run: () => props.onRestoreAgent(agent.name) },
          { label: "彻底删除 Agent", danger: true, run: () => props.onDeleteAgent(agent.name, true) },
        ]} /></div>)}</section>}
  </nav>;
}
