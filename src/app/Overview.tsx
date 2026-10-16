import { IconAgent, IconArrowDown, IconGear, IconPlus } from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";

interface Props {
  agents: AgentDefinition[];
  agent?: AgentDefinition;
  sessions: Record<string, SessionSummaryView[]>;
  running: Set<string>;
  onAgent: (name: string) => void;
  onSession: (name: string, id: string) => void;
  onNewSession: (name: string) => void;
  onConfigure: (name: string) => void;
  onCreate: () => void;
}

function lastActive(timestamp: number): string {
  return timestamp ? new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(timestamp) : "尚无记录";
}

export default function Overview({ agents, agent, sessions, running, onAgent, onSession, onNewSession, onConfigure, onCreate }: Props) {
  const visible = agent ? [agent] : agents;
  const recent = visible.flatMap(item => (sessions[item.name] ?? []).map(session => ({ agent: item, session })))
    .sort((a, b) => b.session.lastActive - a.session.lastActive);
  const runningCount = [...running].filter(key => !agent || key.startsWith(`${agent.name}\u0000`)).length;
  const currentRunning = recent.filter(item => running.has(`${item.agent.name}\u0000${item.session.id}`));
  const renderSession = ({ agent: owner, session }: typeof recent[number]) => <button type="button" key={`${owner.name}:${session.id}`}
    className="activity-row" onClick={() => onSession(owner.name, session.id)}>
    <span className={`activity-dot${running.has(`${owner.name}\u0000${session.id}`) ? " running" : ""}`} />
    <span className="activity-name"><b>{session.title || "未命名会话"}</b><small>{owner.name} · {session.messageCount} 条消息</small></span>
    <time>{lastActive(session.lastActive)}</time><span className="activity-arrow">↗</span>
  </button>;
  return <div className="overview-page">
    <header className="overview-heading">
      <div className="overview-eyebrow"><span className="brand-symbol"><IconAgent /></span>{agent ? "AGENT OVERVIEW" : "YOUR WORKSPACE"}</div>
      <h1>{agent?.name ?? "从一个 Agent 开始"}</h1>
      <p>{agent?.description || (agent ? "为这个 Agent 开启一段新的工作，或接着上次继续。" : "你的助手、上下文与工作，都在这里。")}</p>
      <div className="overview-actions">
        {agent ? <>
          <button type="button" className="btn primary" onClick={() => onNewSession(agent.name)}><IconPlus />新建会话</button>
          {recent[0] && <button type="button" className="btn" onClick={() => onSession(agent.name, recent[0].session.id)}><IconArrowDown />继续最近会话</button>}
          <button type="button" className="btn ghost" onClick={() => onConfigure(agent.name)}><IconGear />配置与测试</button>
        </> : <button type="button" className="btn primary" onClick={onCreate}><IconPlus />新建 Agent</button>}
      </div>
    </header>
    <div className="overview-summary">
      <span><i className={`activity-dot${runningCount ? " running" : ""}`} />{runningCount ? `${runningCount} 条会话运行中` : "准备就绪"}</span>
      <span>{agent ? `${recent.length} 条会话` : `${agents.length} 个 Agent`}</span>
      <span>本地优先 · 一切皆文件</span>
    </div>
    {agent && <section className="agent-facts" aria-label="Agent 信息">
      <div><span>工作目录</span><strong title={agent.workspace ?? undefined}>{agent.workspace || "Agent 内置工作区"}</strong></div>
      <div><span>默认模型</span><strong>{agent.provider?.id || agent.model || "尚未配置"}</strong></div>
      <div><span>工具与权限</span><strong>{agent.permissions.tools.length} 个工具 · {{ "read-only": "只读", "workspace-write": "工作区可写", "danger-full-access": "完全访问" }[agent.permissions.sandbox]}</strong></div>
    </section>}
    {!agent && <section className="overview-section">
      <div className="section-heading"><h2>你的 Agent</h2><span>为长期工作保留上下文</span></div>
      <div className="agent-cards">{agents.map(item => <button type="button" className="agent-card" key={item.name} onClick={() => onAgent(item.name)}>
        <span className="agent-avatar"><IconAgent /></span><span className="agent-card-copy"><b>{item.name}</b><small>{item.description || "准备好开始新的工作"}</small>
          <span>{item.subagent ? "Subagent" : "Agent"} · {(sessions[item.name] ?? []).length} 条会话</span></span>
        {[...running].some(key => key.startsWith(`${item.name}\u0000`)) && <span className="activity-dot running" aria-label="正在运行" />}
        <span className="activity-arrow">↗</span>
      </button>)}</div>
      {!agents.length && <div className="overview-empty"><IconAgent /><h3>创建你的第一个 Agent</h3><p>绑定工作目录与模型，让指令、技能和记忆随工作积累。</p><button type="button" className="btn" onClick={onCreate}>创建 Agent</button></div>}
    </section>}
    {currentRunning.length > 0 && <section className="overview-section"><div className="section-heading"><h2>正在进行</h2><span>切换页面后继续运行</span></div>{currentRunning.map(renderSession)}</section>}
    <section className="overview-section"><div className="section-heading"><h2>最近活动</h2><span>{agent ? "继续已有的工作" : "来自所有 Agent"}</span></div>
      {recent.slice(0, 8).map(renderSession)}
      {!recent.length && <div className="overview-empty compact"><p>还没有会话。{agent ? "新建会话，开始这位 Agent 的第一份工作。" : "选择一个 Agent，开启新的工作。"}</p></div>}
    </section>
    <footer className="overview-footer">随时按 <kbd>Ctrl / ⌘ K</kbd> 搜索 Agent、会话和操作</footer>
  </div>;
}
