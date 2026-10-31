import { useCallback, useEffect, useRef, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { invoke, onHostReconnect } from "../platform";
import type { SessionRuntime } from "../session-runtime";
import type { SessionInfoView } from "../types";
import type { SessionRunStatus } from "./session-status";

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
  /** 此刻在看的会话（chat 视图且已有会话 id）；完成时据此判断要不要记未读。 */
  viewedSessionRef: { current: { agentName: string; sessionId: string | null } | null };
}

export interface HostEventState {
  runningSessions: Set<string>;
  /** 身份稳定（ChatView 的 effect 以它为依赖）：运行态变化按会话合并。 */
  handleRunningChange: () => void;
  isConversationRunning: (agentName: string, sessionId: string | null) => boolean;
  agentHasRunning: (agentName: string) => boolean;
  /** 会话的运行状态（会话标题颜色用）；从未跑过 / 未知的会话返回 undefined。 */
  sessionStatus: (agentName: string, sessionId: string | null) => SessionRunStatus | undefined;
  /** 完成时不在看的会话：标题染蓝，直到被打开。 */
  sessionUnread: (agentName: string, sessionId: string | null) => boolean;
  /** 打开会话即视为已读；进行中的新一轮也会清掉上一次的未读。 */
  markSessionRead: (agentName: string, sessionId: string) => void;
  /** 用户主动停止后标红；正常结束仍由 agent_end 记为完成。 */
  markSessionInterrupted: (agentName: string, sessionId: string) => void;
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
    viewedSessionRef,
  } = options;

  /**
   * 正在跑一轮的会话集合，键是 `conversationKey(Agent, 会话 id)`。
   * 多会话并发下运行态是**集合**：同一个 Agent 可以有多条会话在跑、不同 Agent 也可以，
   * 每条各自一个运行守卫。这个集合只用于显示（侧栏标记 / 底部计数）——守卫在核心。
   */
  const [runningSessions, setRunningSessions] = useState<Set<string>>(() => new Set());
  /**
   * 每个会话最近一次运行状态。运行态变化很少（start / retry / end / error），
   * 不随流式 token 更新；会话标题颜色只读这张表。
   */
  const [sessionStatuses, setSessionStatuses] = useState<Map<string, SessionRunStatus>>(
    () => new Map(),
  );
  /**
   * 完成时用户没在看这条会话 → 未读（标题染蓝）；打开或开始新一轮即清掉。
   * 与运行态一样只存在当前进程。
   */
  const [unreadSessions, setUnreadSessions] = useState<Set<string>>(() => new Set());
  /** 状态表的同步镜像：事件回调里要立刻读到最新值（agent_end / 重连同步）。 */
  const statusesRef = useRef<Map<string, SessionRunStatus>>(sessionStatuses);
  const runningRequestRef = useRef(0);

  /** 立刻向核心核对一次运行态（新会话拿到 id 前 / 事件驱动变化时用）。 */
  const syncRunningRef = useRef<() => void>(() => {});

  const runningKey = useCallback(
    (agentName: string, sessionId: string) => `${agentName}\u0000${sessionId}`,
    [],
  );

  const applyStatuses = useCallback(
    (update: (previous: Map<string, SessionRunStatus>) => Map<string, SessionRunStatus>) => {
      const next = update(statusesRef.current);
      statusesRef.current = next;
      setSessionStatuses(next);
    },
    [],
  );

  const markStatus = useCallback(
    (agentName: string, sessionId: string, status: SessionRunStatus) => {
      const key = runningKey(agentName, sessionId);
      applyStatuses((previous) => {
        if (previous.get(key) === status) return previous;
        const next = new Map(previous);
        next.set(key, status);
        return next;
      });
    },
    [runningKey, applyStatuses],
  );

  /** 重试退避结束、新一轮内容开始时，标题从黄色回到绿色。 */
  const resumeFromRetry = useCallback(
    (agentName: string, sessionId: string) => {
      const key = runningKey(agentName, sessionId);
      applyStatuses((previous) => {
        if (previous.get(key) !== "retrying") return previous;
        const next = new Map(previous);
        next.set(key, "running");
        return next;
      });
    },
    [runningKey, applyStatuses],
  );

  const isViewed = useCallback(
    (agentName: string, sessionId: string) => {
      const viewed = viewedSessionRef.current;
      return viewed !== null && viewed.agentName === agentName && viewed.sessionId === sessionId;
    },
    [viewedSessionRef],
  );

  const markSessionRead = useCallback(
    (agentName: string, sessionId: string) => {
      const key = runningKey(agentName, sessionId);
      setUnreadSessions((previous) => {
        if (!previous.has(key)) return previous;
        const next = new Set(previous);
        next.delete(key);
        return next;
      });
    },
    [runningKey],
  );

  const markUnread = useCallback(
    (agentName: string, sessionId: string) => {
      const key = runningKey(agentName, sessionId);
      setUnreadSessions((previous) => {
        if (previous.has(key)) return previous;
        const next = new Set(previous);
        next.add(key);
        return next;
      });
    },
    [runningKey],
  );

  /**
   * 正常收尾：已经标红（错误 / 中断）的会话不允许被 agent_end 洗成完成；
   * 只有真正从运行态落到完成的、且用户没在看的会话才算未读。
   */
  const completeRun = useCallback(
    (agentName: string, sessionId: string) => {
      const key = runningKey(agentName, sessionId);
      const current = statusesRef.current.get(key);
      if (current === "error" || current === "completed") return;
      applyStatuses((previous) => {
        const next = new Map(previous);
        next.set(key, "completed");
        return next;
      });
      if (!isViewed(agentName, sessionId)) markUnread(agentName, sessionId);
    },
    [runningKey, applyStatuses, isViewed, markUnread],
  );

  const handleRunningChange = useCallback(() => { syncRunningRef.current(); }, []);

  const isConversationRunning = useCallback(
    (agentName: string, sessionId: string | null) =>
      sessionId !== null && runningSessions.has(runningKey(agentName, sessionId)),
    [runningSessions, runningKey],
  );

  const agentHasRunning = (agentName: string) =>
    Array.from(runningSessions).some((key) => key.startsWith(`${agentName}\u0000`));

  const sessionStatus = useCallback(
    (agentName: string, sessionId: string | null) =>
      sessionId === null ? undefined : sessionStatuses.get(runningKey(agentName, sessionId)),
    [sessionStatuses, runningKey],
  );

  const sessionUnread = useCallback(
    (agentName: string, sessionId: string | null) =>
      sessionId !== null && unreadSessions.has(runningKey(agentName, sessionId)),
    [unreadSessions, runningKey],
  );

  const markSessionInterrupted = useCallback(
    (agentName: string, sessionId: string) => markStatus(agentName, sessionId, "error"),
    [markStatus],
  );

  useEffect(() => {
    let active = true;
    const sync = () => {
      const request = ++runningRequestRef.current;
      void invoke("session_infos")
        .then((infos) => {
          if (!active || request !== runningRequestRef.current) return;
          const running = new Set(
            infos
              .filter((info) => info.running)
              .map((info) => `${info.agentName}\u0000${info.sessionId}`),
          );
          setRunningSessions(running);
          const previous = statusesRef.current;
          let next: Map<string, SessionRunStatus> | null = null;
          for (const key of running) {
            if (previous.has(key)) continue;
            next ??= new Map(previous);
            next.set(key, "running");
          }
          // 错过 agent_end 时（例如宿主重连），以核心的运行态兜底收尾。
          const settled: string[] = [];
          for (const [key, status] of previous) {
            if ((status !== "running" && status !== "retrying") || running.has(key)) continue;
            next ??= new Map(previous);
            next.set(key, "completed");
            settled.push(key);
          }
          if (next) {
            statusesRef.current = next;
            setSessionStatuses(next);
          }
          for (const key of settled) {
            const [agentName, sessionId] = key.split("\u0000");
            if (agentName && sessionId && !isViewed(agentName, sessionId)) {
              markUnread(agentName, sessionId);
            }
          }
        })
        .catch(() => {});
    };
    syncRunningRef.current = sync;
    sync();
    const unsubscribe = onHostReconnect(() => {
      sync();
      void sessionRuntime.resynchronize();
      if (selectedRef.current) void refreshSessions([selectedRef.current]);
    });
    return () => {
      active = false;
      unsubscribe();
    };
  }, [runningSessions.size, isViewed, markUnread]);

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
      if (frame.type === "session-error") {
        markStatus(frame.payload.agentName, frame.payload.sessionId, "error");
        return;
      }
      if (frame.type !== "agent-event") return;
      const meta = frame.payload;
      const event = meta.event;
      switch (event.type) {
        case "agent_start":
          markStatus(meta.agentName, meta.sessionId, "running");
          // 新一轮开始，上一次完成的未读提醒不再成立。
          markSessionRead(meta.agentName, meta.sessionId);
          break;
        case "retry_start":
          markStatus(meta.agentName, meta.sessionId, "retrying");
          break;
        case "message_start":
        case "tool_execution_start":
          resumeFromRetry(meta.agentName, meta.sessionId);
          break;
        case "agent_end":
          completeRun(meta.agentName, meta.sessionId);
          break;
        default:
          break;
      }
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
    markStatus,
    resumeFromRetry,
    completeRun,
    markSessionRead,
  ]);

  const forgetRunningSession = useCallback((agentName: string, sessionId: string) => {
    const key = runningKey(agentName, sessionId);
    setRunningSessions((previous) => {
      if (!previous.has(key)) return previous;
      const next = new Set(previous);
      next.delete(key);
      return next;
    });
    applyStatuses((previous) => {
      if (!previous.has(key)) return previous;
      const next = new Map(previous);
      next.delete(key);
      return next;
    });
    setUnreadSessions((previous) => {
      if (!previous.has(key)) return previous;
      const next = new Set(previous);
      next.delete(key);
      return next;
    });
  }, [runningKey, applyStatuses]);

  const forgetRunningAgent = useCallback((agentName: string) => {
    const prefix = `${agentName}\u0000`;
    setRunningSessions((previous) => {
      const next = new Set([...previous].filter((key) => !key.startsWith(prefix)));
      return next.size === previous.size ? previous : next;
    });
    applyStatuses((previous) => {
      const next = new Map([...previous].filter(([key]) => !key.startsWith(prefix)));
      return next.size === previous.size ? previous : next;
    });
    setUnreadSessions((previous) => {
      const next = new Set([...previous].filter((key) => !key.startsWith(prefix)));
      return next.size === previous.size ? previous : next;
    });
  }, [applyStatuses]);

  return {
    runningSessions,
    handleRunningChange,
    isConversationRunning,
    agentHasRunning,
    sessionStatus,
    sessionUnread,
    markSessionRead,
    markSessionInterrupted,
    forgetRunningSession,
    forgetRunningAgent,
  };
}
