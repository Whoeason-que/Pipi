import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { formatRuntimeError, isAuthSilentError } from "../chat-runtime";
import { invoke } from "../platform";
import type { AgentDefinition, SessionSummaryView } from "../types";

export interface SessionListState {
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  refresh: () => Promise<void>;
  refreshSessions: (names: string[]) => Promise<void>;
  refreshArchived: () => Promise<void>;
  /** 删除 / 归档一个 Agent 后清掉它的会话缓存（避免同名重建后残留旧数据）。 */
  removeAgentSessions: (agentName: string) => void;
  sessionsFor: (agent: AgentDefinition) => SessionSummaryView[];
  sessionTotal: number;
}

/**
 * Agent 列表、每个 Agent 的会话列表与归档 Agent 的加载。
 *
 * 每个请求带自增编号：旧响应回来时编号已不是最新就丢弃，并发刷新不会把
 * 新数据覆盖回旧状态。`setError` 由调用方传入 safeSetError（对核心的
 * 「运行中」拒绝做可执行化改写）。
 */
export function useSessionList(
  setError: (message: string | null) => void,
): SessionListState {
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [archivedAgents, setArchivedAgents] = useState<AgentDefinition[]>([]);
  const [sessionsByAgent, setSessionsByAgent] = useState<
    Record<string, SessionSummaryView[]>
  >({});
  // 供 refreshSessions 读取最新 Agent 名单；refresh 同步写入，不等重渲染。
  const agentsRef = useRef<AgentDefinition[]>([]);
  const sessionListRequestRef = useRef(new Map<string, number>());
  const agentRequestRef = useRef(0);
  agentsRef.current = agents;

  const refreshSessions = useCallback(async (names: string[]) => {
    const results = await Promise.all(names.map(async (name) => {
      const requestId = (sessionListRequestRef.current.get(name) ?? 0) + 1;
      sessionListRequestRef.current.set(name, requestId);
      try {
        const sessions = await invoke("list_sessions", { agentName: name });
        return { name, requestId, sessions, failed: false };
      } catch {
        return { name, requestId, sessions: null, failed: true };
      }
    }));
    setSessionsByAgent((previous) => {
      const known = new Set(agentsRef.current.map((agent) => agent.name));
      const next = Object.fromEntries(Object.entries(previous).filter(([name]) => known.has(name)));
      for (const result of results) {
        if (
          known.has(result.name)
          && result.requestId === sessionListRequestRef.current.get(result.name)
          && result.sessions
        ) next[result.name] = result.sessions;
      }
      return next;
    });
    const failedNames = results
      .filter((result) => result.failed
        && result.requestId === sessionListRequestRef.current.get(result.name))
      .map((result) => result.name);
    if (failedNames.length) setError(`会话列表加载失败：${failedNames.join("、")}（保留旧数据）`);
  }, [setError]);

  const refresh = useCallback(async () => {
    const requestId = ++agentRequestRef.current;
    try {
      const nextAgents = await invoke("list_agents");
      if (requestId !== agentRequestRef.current) return;
      agentsRef.current = nextAgents;
      setAgents(nextAgents);
      setError(null);
    } catch (errorValue) {
      if (requestId === agentRequestRef.current) {
        if (isAuthSilentError(errorValue)) return;
        setError(formatRuntimeError(errorValue));
      }
    }
  }, [setError]);

  const refreshArchived = useCallback(async () => {
    try {
      const next = await invoke("list_agents", { archived: true });
      setArchivedAgents(next);
    } catch (errorValue) {
      if (isAuthSilentError(errorValue)) return;
      setError(formatRuntimeError(errorValue));
    }
  }, [setError]);

  const removeAgentSessions = useCallback((agentName: string) => {
    setSessionsByAgent((previous) => {
      const next = { ...previous };
      delete next[agentName];
      return next;
    });
  }, []);

  // agents 变化后拉取各 Agent 的会话列表；空列表也要清理旧数据。
  useEffect(() => {
    if (agents.length === 0) {
      setSessionsByAgent({});
      return;
    }
    void refreshSessions(agents.map((agent) => agent.name));
  }, [agents, refreshSessions]);

  const sessionsFor = useCallback((agent: AgentDefinition): SessionSummaryView[] =>
    sessionsByAgent[agent.name] ?? [], [sessionsByAgent]);

  const sessionTotal = useMemo(
    () => Object.values(sessionsByAgent).reduce((total, list) => total + list.length, 0),
    [sessionsByAgent],
  );

  return {
    agents,
    archivedAgents,
    sessionsByAgent,
    refresh,
    refreshSessions,
    refreshArchived,
    removeAgentSessions,
    sessionsFor,
    sessionTotal,
  };
}
