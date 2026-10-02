// 搜索分区：跨 Agent 与会话的平面检索。
// 搜索不再是「粘在列表顶部的过滤器」——列表始终完整显示，检索有自己的面板，
// 点击结果直接跳转（Agent → 详情/工作台，会话 → 打开该会话）。

import { useMemo } from "react";
import { IconClose, IconSearch } from "./icons";
import type { AgentDefinition, SessionSummaryView } from "./types";

/** 会话命中上限：命中整屏时靠关键字收敛，比无限渲染滚动列表更实用。 */
const MAX_SESSION_HITS = 60;

interface SessionHit {
  agentName: string;
  session: SessionSummaryView;
  running: boolean;
}

interface SearchPanelProps {
  query: string;
  onQueryChange: (query: string) => void;
  agents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  isRunning: (agentName: string, sessionId: string) => boolean;
  onSelectAgent: (agentName: string) => void;
  onOpenSession: (agentName: string, sessionId: string) => void;
}

export function SearchPanel({
  query,
  onQueryChange,
  agents,
  sessionsByAgent,
  isRunning,
  onSelectAgent,
  onOpenSession,
}: SearchPanelProps) {
  const normalized = query.trim().toLowerCase();
  const { agentHits, sessionHits, totalSessions } = useMemo(() => {
    if (!normalized) {
      return { agentHits: [] as AgentDefinition[], sessionHits: [] as SessionHit[], totalSessions: 0 };
    }
    const agentHits = agents.filter((agent) => (
      agent.name.toLowerCase().includes(normalized)
      || agent.description.toLowerCase().includes(normalized)
    ));
    // Agent 命中时它的会话都算命中（用户按 Agent 名找会话的场景）。
    const hitAgentNames = new Set(agentHits.map((agent) => agent.name));
    const all: SessionHit[] = [];
    for (const agent of agents) {
      const wholeAgent = hitAgentNames.has(agent.name);
      for (const session of sessionsByAgent[agent.name] ?? []) {
        if (wholeAgent || session.title.toLowerCase().includes(normalized)) {
          all.push({
            agentName: agent.name,
            session,
            running: isRunning(agent.name, session.id),
          });
        }
      }
    }
    all.sort((a, b) => b.session.lastActive - a.session.lastActive);
    return {
      agentHits,
      sessionHits: all.slice(0, MAX_SESSION_HITS),
      totalSessions: all.length,
    };
  }, [agents, normalized, sessionsByAgent, isRunning]);

  const truncated = totalSessions - sessionHits.length;
  const empty = Boolean(normalized) && agentHits.length === 0 && sessionHits.length === 0;

  return (
    <>
      <div className="sb-filter">
        <div className="search">
          <IconSearch />
          <input
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            placeholder="搜索 Agent / 会话…"
            aria-label="搜索 Agent 与会话"
            autoComplete="off"
            spellCheck={false}
            // 触屏不自动聚焦：不然切到搜索分区就弹软键盘
            autoFocus={window.matchMedia("(hover: hover)").matches}
          />
          {query && (
            <button
              type="button"
              className="search-clear"
              title="清空搜索"
              aria-label="清空搜索"
              onClick={() => onQueryChange("")}
            >
              <IconClose />
            </button>
          )}
        </div>
      </div>

      <div className="search-panel">
        {!normalized ? (
          <p className="search-tip">按 Agent 名称、描述或会话标题检索；点击结果直接跳转。</p>
        ) : (
          <>
            {agentHits.length > 0 && (
              <div className="sb-sec search-group">
                <span>AGENTS</span>
                <span className="spacer" />
                <span className="sb-count">{agentHits.length}</span>
              </div>
            )}
            {agentHits.map((agent) => (
              <button
                key={agent.name}
                type="button"
                className="search-item"
                title={agent.description || agent.name}
                onClick={() => onSelectAgent(agent.name)}
              >
                <span className="search-item-name">
                  {agent.name}
                  {agent.subagent && <span className="search-item-tag">SUBS</span>}
                </span>
                <span className="search-item-meta">{agent.description || "（暂无描述）"}</span>
              </button>
            ))}

            {sessionHits.length > 0 && (
              <div className="sb-sec search-group">
                <span>会话</span>
                <span className="spacer" />
                <span className="sb-count">
                  {sessionHits.length}
                  {truncated > 0 ? `+${truncated}` : ""}
                </span>
              </div>
            )}
            {sessionHits.map((hit) => (
              <button
                key={`${hit.agentName}\u0000${hit.session.id}`}
                type="button"
                className="search-item"
                title={`${hit.agentName} · ${hit.session.title}`}
                onClick={() => onOpenSession(hit.agentName, hit.session.id)}
              >
                <span className="search-item-name">
                  {hit.running && <span className="search-item-live" title="正在运行" aria-hidden="true" />}
                  {hit.session.title || "（无标题）"}
                </span>
                <span className="search-item-meta">{hit.agentName}</span>
              </button>
            ))}

            {truncated > 0 && (
              <p className="search-tip">另有 {truncated} 条会话结果，输入更多关键字以缩小范围。</p>
            )}
            {empty && <div className="session pending">没有匹配的 Agent 或会话</div>}
          </>
        )}
      </div>
    </>
  );
}
