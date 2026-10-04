import { useCallback, useEffect, useRef, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { invoke } from "../platform";
import type { SessionRuntime } from "../session-runtime";
import type { SessionInfoView } from "../types";

export interface HostEventOptions {
  sessionRuntime: SessionRuntime;
  refreshSessions: (names: string[]) => Promise<void>;
  /** 可执行化的错误提示（会话运行启动失败等）。 */
  safeSetError: (message: string | null) => void;
  /** 原始 setError：session_info 拉取失败按核心原文展示，不做改写。 */
  setError: (message: string | null) => void;
  setActiveSession: (info: SessionInfoView | null) => void;
  selectedRef: { current: string | null };
  activeSessionRef: { current: SessionInfoView | null };
  blockedSessionIdsRef: { current: Set<string> };
  sessionInfoRequestRef: { current: number };
}

export interface HostEventState {
  runningSessions: Set<string>;
  /** 身份稳定（ChatView 的 effect 以它为依赖）：运行态变化按会话合并。 */
  handleRunningChange: () => void;
  isConversationRunning: (agentName: string, sessionId: string | null) => boolean;
  agentHasRunning: (agentName: string) => boolean;
  forgetRunningSession: (agentName: string, sessionId: string) => void;
  forgetRunningAgent: (agentName: string) => void;
}

/**
 * 唯一的宿主事件订阅（sessionRuntime.subscribe）与运行态集合。
 *
 * `agent_start` / `agent_end` 只触发一次 `session_infos` 核对；会话列表由
 * `session-changed` / `agent_end` 刷新。视图只订阅投影，不各自注册宿主事件。
 */
export function useHostEvents(options: HostEventOptions): HostEventState {
  const {
    sessionRuntime,
    refreshSessions,
    safeSetError,
    setError,
    setActiveSession,
    selectedRef,
    activeSessionRef,
    blockedSessionIdsRef,
    sessionInfoRequestRef,
  } = options;

  /**
   * 正在跑一轮的会话集合，键是 `conversationKey(Agent, 会话 id)`。
   * 多会话并发下运行态是**集合**：同一个 Agent 可以有多条会话在跑、不同 Agent 也可以，
   * 每条各自一个运行守卫。这个集合只用于显示（侧栏标记 / 底部计数）——守卫在核心。
   */
  const [runningSessions, setRunningSessions] = useState<Set<string>>(() => new Set());
  const runningRequestRef = useRef(0);

  /** 立刻向核心核对一次运行态（新会话拿到 id 前 / 事件驱动变化时用）。 */
  const syncRunningRef = useRef<() => void>(() => {});

  const runningKey = useCallback(
    (agentName: string, sessionId: string) => `${agentName}\u0000${sessionId}`,
    [],
  );

  const handleRunningChange = useCallback(() => { syncRunningRef.current(); }, []);

  const isConversationRunning = useCallback(
    (agentName: string, sessionId: string | null) =>
      sessionId !== null && runningSessions.has(runningKey(agentName, sessionId)),
    [runningSessions, runningKey],
  );

  const agentHasRunning = (agentName: string) =>
    Array.from(runningSessions).some((key) => key.startsWith(`${agentName}\u0000`));

  useEffect(() => {
    let active = true;
    const sync = () => {
      const request = ++runningRequestRef.current;
      void invoke("session_infos")
        .then((infos) => {
          if (!active || request !== runningRequestRef.current) return;
          setRunningSessions(
            new Set(
              infos
                .filter((info) => info.running)
                .map((info) => `${info.agentName}\u0000${info.sessionId}`),
            ),
          );
        })
        .catch(() => {});
    };
    syncRunningRef.current = sync;
    sync();
    if (runningSessions.size === 0) {
      return () => {
        active = false;
      };
    }
    const timer = window.setInterval(sync, 2000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [runningSessions.size]);

  // 只订阅一次全局完成事件，读取 ref 避免因 Agent 列表更新反复注册监听器。
  useEffect(() => {
    let active = true;
    void sessionRuntime.start().catch(error => { if (active) safeSetError(formatRuntimeError(error)); });
    const unsubscribe = sessionRuntime.subscribe(frame => {
      if (!active) return;
      if (frame.type === "session-changed") {
        void refreshSessions([frame.payload.agentName]);
        return;
      }
      if (frame.type !== "agent-event") return;
      const meta = frame.payload;
      const event = meta.event;
      if (event.type === "agent_start" || event.type === "agent_end") syncRunningRef.current();
      if (event.type !== "agent_end") return;
      void refreshSessions([meta.agentName]);
      if (
        blockedSessionIdsRef.current.has(meta.sessionId)
          || meta.agentName !== selectedRef.current
          || (activeSessionRef.current?.sessionId
            && meta.sessionId !== activeSessionRef.current.sessionId)
          // 已结算的旧 run 的迟到结束事件不再触发刷新
          || (activeSessionRef.current?.runId != null
            && meta.runId < activeSessionRef.current.runId)
      ) return;
      const targetAgent = meta.agentName;
      const targetSession = meta.sessionId;
      if (targetAgent && targetSession) {
        const infoRequestId = ++sessionInfoRequestRef.current;
        void invoke("session_info", {
          agentName: targetAgent,
          sessionId: targetSession,
        })
          .then((info) => {
            if (active && infoRequestId === sessionInfoRequestRef.current) setActiveSession(info);
          })
          .catch((errorValue) => {
            if (active) setError(formatRuntimeError(errorValue));
          });
      }
    });
    return () => {
      active = false;
      unsubscribe();
      sessionRuntime.disconnect();
    };
  }, [
    sessionRuntime,
    refreshSessions,
    safeSetError,
    setError,
    setActiveSession,
    selectedRef,
    activeSessionRef,
    blockedSessionIdsRef,
    sessionInfoRequestRef,
  ]);

  const forgetRunningSession = useCallback((agentName: string, sessionId: string) => {
    setRunningSessions((previous) => {
      const key = runningKey(agentName, sessionId);
      if (!previous.has(key)) return previous;
      const next = new Set(previous);
      next.delete(key);
      return next;
    });
  }, [runningKey]);

  const forgetRunningAgent = useCallback((agentName: string) => {
    const prefix = `${agentName}\u0000`;
    setRunningSessions((previous) => {
      const next = new Set([...previous].filter((key) => !key.startsWith(prefix)));
      return next.size === previous.size ? previous : next;
    });
  }, []);

  return {
    runningSessions,
    handleRunningChange,
    isConversationRunning,
    agentHasRunning,
    forgetRunningSession,
    forgetRunningAgent,
  };
}
