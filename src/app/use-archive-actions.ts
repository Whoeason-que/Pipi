import type { Dispatch, SetStateAction } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { invoke } from "../platform";
import type { SessionRuntime } from "../session-runtime";
import type { SessionInfoView, SessionSummaryView } from "../types";

export interface ConfirmRequest {
  title: string;
  message: string;
  action: () => Promise<void>;
}

export interface ArchiveActionOptions {
  sessionRuntime: SessionRuntime;
  safeSetError: (message: string | null) => void;
  setConfirmState: Dispatch<SetStateAction<ConfirmRequest | null>>;
  refresh: () => Promise<void>;
  refreshArchived: () => Promise<void>;
  refreshSessions: (names: string[]) => Promise<void>;
  removeAgentSessions: (agentName: string) => void;
  selectedRef: { current: string | null };
  activeSessionRef: { current: SessionInfoView | null };
  invalidateNavigation: () => void;
  chatOpen: boolean;
  viewSessionId: string | null;
  setSelected: Dispatch<SetStateAction<string | null>>;
  setChatOpen: Dispatch<SetStateAction<boolean>>;
  setActiveSession: Dispatch<SetStateAction<SessionInfoView | null>>;
  setViewSessionId: Dispatch<SetStateAction<string | null>>;
  setChatKey: Dispatch<SetStateAction<number>>;
  forgetRunningSession: (agentName: string, sessionId: string) => void;
  forgetRunningAgent: (agentName: string) => void;
  archivedSessionsExpanded: Record<string, boolean>;
  setArchivedSessions: Dispatch<SetStateAction<Record<string, SessionSummaryView[]>>>;
  setArchivedSessionsExpanded: Dispatch<SetStateAction<Record<string, boolean>>>;
}

export interface ArchiveActionState {
  archiveAgent: (name: string) => Promise<void>;
  restoreAgent: (name: string) => Promise<void>;
  deleteAgent: (name: string, archived: boolean) => Promise<void>;
  archiveSession: (agentName: string, sessionId: string) => Promise<void>;
  deleteSession: (agentName: string, sessionId: string) => Promise<void>;
  toggleArchivedSessions: (agentName: string) => Promise<void>;
  restoreSession: (agentName: string, sessionId: string) => Promise<void>;
  deleteArchivedSession: (agentName: string, sessionId: string) => Promise<void>;
}

/**
 * 归档 / 恢复 / 删除（Agent 与会话）。
 *
 * 归档 = 移到 .archive/（文件即真相）；删除不可恢复，一律 confirm 后才执行。
 * 归档/删除当前正在查看的会话后，同步清理前端身份、run 回放缓存与运行态标记，
 * 避免界面继续把已归档/已删除的会话当成可发送目标。
 */
export function useArchiveActions(options: ArchiveActionOptions): ArchiveActionState {
  const {
    sessionRuntime,
    safeSetError,
    setConfirmState,
    refresh,
    refreshArchived,
    refreshSessions,
    removeAgentSessions,
    selectedRef,
    activeSessionRef,
    invalidateNavigation,
    chatOpen,
    viewSessionId,
    setSelected,
    setChatOpen,
    setActiveSession,
    setViewSessionId,
    setChatKey,
    forgetRunningSession,
    forgetRunningAgent,
    archivedSessionsExpanded,
    setArchivedSessions,
    setArchivedSessionsExpanded,
  } = options;

  const finishSessionMutation = (agentName: string, sessionId: string) => {
    sessionRuntime.forget(agentName, sessionId);
    forgetRunningSession(agentName, sessionId);
    if (
      selectedRef.current !== agentName
      || !chatOpen
      || viewSessionId !== sessionId
    ) {
      return;
    }
    invalidateNavigation();
    activeSessionRef.current = null;
    setActiveSession(null);
    setViewSessionId(null);
    setChatKey((key) => key + 1);
  };

  const forgetAgentRuntime = (agentName: string) => {
    sessionRuntime.forgetAgent(agentName);
    forgetRunningAgent(agentName);
    if (selectedRef.current !== agentName) return;
    invalidateNavigation();
    activeSessionRef.current = null;
    setActiveSession(null);
    setViewSessionId(null);
    setChatOpen(false);
  };

  const archiveAgent = async (name: string) => {
    try {
      await invoke("archive_agent", { name });
      forgetAgentRuntime(name);
      if (selectedRef.current === name) {
        setSelected(null);
        setChatOpen(false);
      }
      removeAgentSessions(name);
      await Promise.all([refresh(), refreshArchived()]);
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const restoreAgent = async (name: string) => {
    try {
      await invoke("restore_agent", { name });
      await Promise.all([refresh(), refreshArchived()]);
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const deleteAgent = async (name: string, archived: boolean) => {
    const kind = archived ? "已归档 Agent" : "Agent";
    setConfirmState({
      title: `彻底删除${kind}「${name}」？`,
      message:
        "其目录下的会话、技能、记忆与工作区文件将一并删除，此操作不可恢复。",
      action: async () => {
        try {
          await invoke("delete_agent", { name, archived });
          if (!archived) forgetAgentRuntime(name);
          if (!archived && selectedRef.current === name) {
            setSelected(null);
            setChatOpen(false);
          }
          if (!archived) removeAgentSessions(name);
          // Agent 消失后归档会话缓存一并清掉，避免同名重建后残留旧数据
          setArchivedSessions((previous) => {
            const next = { ...previous };
            delete next[name];
            return next;
          });
          setArchivedSessionsExpanded((previous) => {
            const next = { ...previous };
            delete next[name];
            return next;
          });
          await Promise.all([refresh(), refreshArchived()]);
        } catch (errorValue) {
          safeSetError(formatRuntimeError(errorValue));
        }
      },
    });
  };

  const archiveSession = async (agentName: string, sessionId: string) => {
    try {
      await invoke("archive_session", { agentName, sessionId });
      finishSessionMutation(agentName, sessionId);
      await refreshSessions([agentName]);
      // 「已归档」折叠区若正展开，同步回写新归档的会话，避免收起再展开才可见
      if (archivedSessionsExpanded[agentName]) {
        const list = await invoke("list_sessions", {
          agentName,
          archived: true,
        });
        setArchivedSessions((previous) => ({ ...previous, [agentName]: list }));
      }
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const deleteSession = async (agentName: string, sessionId: string) => {
    setConfirmState({
      title: "彻底删除该会话？",
      message: "其记录文件将被删除，此操作不可恢复。",
      action: async () => {
        try {
          await invoke("delete_session", { agentName, sessionId });
          finishSessionMutation(agentName, sessionId);
          await refreshSessions([agentName]);
        } catch (errorValue) {
          safeSetError(formatRuntimeError(errorValue));
        }
      },
    });
  };

  // 归档会话的「已归档」折叠区：展开时才拉取列表（避免每次刷新都读全部归档 JSONL）
  const toggleArchivedSessions = async (agentName: string) => {
    const willOpen = !archivedSessionsExpanded[agentName];
    setArchivedSessionsExpanded((previous) => ({ ...previous, [agentName]: willOpen }));
    if (!willOpen) return;
    try {
      const list = await invoke("list_sessions", {
        agentName,
        archived: true,
      });
      setArchivedSessions((previous) => ({ ...previous, [agentName]: list }));
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const restoreSession = async (agentName: string, sessionId: string) => {
    try {
      await invoke("restore_session", { agentName, sessionId });
      setArchivedSessions((previous) => ({
        ...previous,
        [agentName]: (previous[agentName] ?? []).filter((s) => s.id !== sessionId),
      }));
      await refreshSessions([agentName]);
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const deleteArchivedSession = async (agentName: string, sessionId: string) => {
    setConfirmState({
      title: "彻底删除该归档会话？",
      message: "其记录文件将被删除，此操作不可恢复。",
      action: async () => {
        try {
          await invoke("delete_session", { agentName, sessionId, archived: true });
          setArchivedSessions((previous) => ({
            ...previous,
            [agentName]: (previous[agentName] ?? []).filter((s) => s.id !== sessionId),
          }));
        } catch (errorValue) {
          safeSetError(formatRuntimeError(errorValue));
        }
      },
    });
  };

  return {
    archiveAgent,
    restoreAgent,
    deleteAgent,
    archiveSession,
    deleteSession,
    toggleArchivedSessions,
    restoreSession,
    deleteArchivedSession,
  };
}
