import { useRef, type Dispatch, type SetStateAction } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { invoke } from "../platform";
import type { SessionInfoView } from "../types";

export interface SessionNavigationOptions {
  /** 原始 setError：会话打开/新建失败按核心原文展示。 */
  setError: (message: string | null) => void;
  selectedRef: { current: string | null };
  activeSessionRef: { current: SessionInfoView | null };
  onOpenChat: (agentName: string, sessionId: string | null) => void;
  setSidebarOpen: Dispatch<SetStateAction<boolean>>;
  setExpandedAgents: Dispatch<SetStateAction<Record<string, boolean>>>;
  setActiveSession: Dispatch<SetStateAction<SessionInfoView | null>>;
  setChatKey: Dispatch<SetStateAction<number>>;
}

export interface SessionNavigationState {
  openSession: (agentName: string, sessionId: string) => Promise<void>;
  startNewSession: (agentName: string) => Promise<void>;
  createSessionFromChat: (previousSessionId?: string) => Promise<boolean>;
  /** 让在途的打开/新建请求作废（Archive / 切换 Agent / 返回时调用）。 */
  invalidateNavigation: () => void;
  blockedSessionIdsRef: { current: Set<string> };
  sessionInfoRequestRef: { current: number };
}

/**
 * 会话视图的打开 / 新建导航。
 *
 * 请求按自增编号定序、经一条串行队列执行：新请求一发起旧的立即作废，
 * 但已在执行的 IPC 不会被并发打断；同一条会话的迟到回包也按编号丢弃。
 * 每条会话是独立的一路——切换与打开不被运行态拦住。
 */
export function useSessionNavigation(
  options: SessionNavigationOptions,
): SessionNavigationState {
  const {
    setError,
    selectedRef,
    activeSessionRef,
    onOpenChat,
    setSidebarOpen,
    setExpandedAgents,
    setActiveSession,
    setChatKey,
  } = options;

  const navigationRequestRef = useRef(0);
  const sessionInfoRequestRef = useRef(0);
  const blockedSessionIdsRef = useRef(new Set<string>());
  const navigationQueueRef = useRef<Promise<void>>(Promise.resolve());

  const invalidateNavigation = () => {
    navigationRequestRef.current += 1;
    sessionInfoRequestRef.current += 1;
  };

  const enqueueNavigation = async (
    requestId: number,
    operation: () => Promise<void>,
  ): Promise<void> => {
    const previous = navigationQueueRef.current;
    let release!: () => void;
    navigationQueueRef.current = new Promise<void>((resolve) => {
      release = resolve;
    });
    await previous;
    try {
      if (requestId === navigationRequestRef.current) await operation();
    } finally {
      release();
    }
  };

  /// 打开（查看）一条会话。会话之间互不影响：别的会话在跑也能打开、切换、
  /// 查看 —— 每条会话是独立的一路。
  const openSession = async (agentName: string, sessionId: string) => {
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    const previousSessionId = activeSessionRef.current?.sessionId;
    if (previousSessionId && previousSessionId !== sessionId) {
      blockedSessionIdsRef.current.add(previousSessionId);
    }
    blockedSessionIdsRef.current.delete(sessionId);
    await enqueueNavigation(requestId, async () => {
      try {
        const snapshot = await invoke("open_session", { agentName, sessionId });
        if (requestId !== navigationRequestRef.current) {
          return;
        }
        onOpenChat(agentName, sessionId);
        setSidebarOpen(false);
        setExpandedAgents((prev) => ({ ...prev, [agentName]: true }));
        setChatKey((key) => key + 1);
        setActiveSession(snapshot.info);
      } catch (errorValue) {
        if (requestId === navigationRequestRef.current) {
          if (previousSessionId && previousSessionId !== sessionId) {
            blockedSessionIdsRef.current.delete(previousSessionId);
          }
          setError(formatRuntimeError(errorValue));
        }
      }
    });
  };

  /// 为某个 Agent 新建一条会话：视图切到「新会话」（还没落文件），
  /// 同时释放它名下**空闲**的会话（正在跑的那条留着，后台继续）。
  const startNewSession = async (agentName: string) => {
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    const previousSessionId = activeSessionRef.current?.sessionId;
    if (previousSessionId) blockedSessionIdsRef.current.add(previousSessionId);
    await enqueueNavigation(requestId, async () => {
      try {
        await invoke("new_session", { agentName });
        if (requestId !== navigationRequestRef.current) return;
        onOpenChat(agentName, null);
        setSidebarOpen(false);
        setExpandedAgents((prev) => ({ ...prev, [agentName]: true }));
        setActiveSession(null);
        setChatKey((key) => key + 1);
      } catch (errorValue) {
        if (requestId === navigationRequestRef.current) {
          if (previousSessionId) blockedSessionIdsRef.current.delete(previousSessionId);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
  };

  /// ChatView 里点「新建会话」：释放该 Agent 的空闲会话（跑着的留着），
  /// 视图切到「新会话」并重挂 ChatView。
  const createSessionFromChat = async (previousSessionId?: string): Promise<boolean> => {
    const agentName = selectedRef.current;
    if (!agentName) return false;
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    if (previousSessionId) blockedSessionIdsRef.current.add(previousSessionId);
    let accepted = false;
    await enqueueNavigation(requestId, async () => {
      try {
        await invoke("new_session", { agentName });
        accepted = requestId === navigationRequestRef.current;
      } catch (errorValue) {
        if (requestId === navigationRequestRef.current) {
          if (previousSessionId) blockedSessionIdsRef.current.delete(previousSessionId);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
    if (!accepted || requestId !== navigationRequestRef.current) return false;
    onOpenChat(agentName, null);
    setActiveSession(null);
    setChatKey((key) => key + 1);
    return true;
  };

  return {
    openSession,
    startNewSession,
    createSessionFromChat,
    invalidateNavigation,
    blockedSessionIdsRef,
    sessionInfoRequestRef,
  };
}
