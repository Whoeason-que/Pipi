import type { Dispatch, SetStateAction } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { useI18n } from "../i18n";
import { invoke } from "../platform";
import type { SessionRuntime } from "../session-runtime";
import type { SessionInfoView } from "../types";
import type { AppView } from "./navigation";

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
  viewRef: { current: AppView };
  onNavigate: (view: AppView) => void;
  setActiveSession: Dispatch<SetStateAction<SessionInfoView | null>>;
  forgetRunningSession: (agentName: string, sessionId: string) => void;
  forgetRunningAgent: (agentName: string) => void;
  forgetNavigationSession: (agentName: string, sessionId: string) => void;
  forgetNavigationAgent: (agentName: string) => void;
}

export interface ArchiveActionState {
  archiveAgent: (name: string) => Promise<void>;
  restoreAgent: (name: string) => Promise<void>;
  deleteAgent: (name: string, archived: boolean) => Promise<void>;
  archiveSession: (agentName: string, sessionId: string) => Promise<void>;
  restoreSession: (agentName: string, sessionId: string) => Promise<void>;
  deleteSession: (agentName: string, sessionId: string, archived: boolean) => Promise<void>;
}

/**
 * Agent 归档 / 恢复 / 删除，以及会话删除。
 *
 * 归档 = 移到 .archive/（文件即真相）；删除不可恢复，一律 confirm 后才执行。
 * 删除当前正在查看的会话后，同步清理前端身份、run 回放缓存与运行态标记。
 */
export function useArchiveActions(options: ArchiveActionOptions): ArchiveActionState {
  const { t } = useI18n();
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
    viewRef,
    onNavigate,
    setActiveSession,
    forgetRunningSession,
    forgetRunningAgent,
    forgetNavigationSession,
    forgetNavigationAgent,
  } = options;

  const finishSessionMutation = (agentName: string, sessionId: string) => {
    sessionRuntime.forget(agentName, sessionId);
    forgetRunningSession(agentName, sessionId);
    forgetNavigationSession(agentName, sessionId);
    if (
      selectedRef.current !== agentName
      || viewRef.current.kind !== "chat"
      || viewRef.current.sessionId !== sessionId
    ) {
      return;
    }
    invalidateNavigation();
    activeSessionRef.current = null;
    setActiveSession(null);
    onNavigate({ kind: "agent", agentName });
  };

  const forgetAgentRuntime = (agentName: string) => {
    sessionRuntime.forgetAgent(agentName);
    forgetRunningAgent(agentName);
    forgetNavigationAgent(agentName);
    if (selectedRef.current !== agentName) return;
    invalidateNavigation();
    activeSessionRef.current = null;
    setActiveSession(null);
    onNavigate({ kind: "home" });
  };

  const archiveAgent = async (name: string) => {
    try {
      await invoke("archive_agent", { name });
      forgetAgentRuntime(name);
      if (selectedRef.current === name) {
        onNavigate({ kind: "home" });
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
    const kind = t(archived ? "app.archivedAgentKind" : "app.agentKind");
    setConfirmState({
      title: t("app.deleteAgentTitle", { kind, name }),
      message: t("app.deleteAgentMessage"),
      action: async () => {
        try {
          await invoke("delete_agent", { name, archived });
          if (!archived) forgetAgentRuntime(name);
          if (!archived && selectedRef.current === name) {
            onNavigate({ kind: "home" });
          }
          if (!archived) removeAgentSessions(name);
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
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const restoreSession = async (agentName: string, sessionId: string) => {
    try {
      await invoke("restore_session", { agentName, sessionId });
      await refreshSessions([agentName]);
    } catch (errorValue) {
      safeSetError(formatRuntimeError(errorValue));
    }
  };

  const deleteSession = async (agentName: string, sessionId: string, archived: boolean) => {
    setConfirmState({
      title: t(archived ? "app.deleteArchivedSessionTitle" : "app.deleteSessionTitle"),
      message: t("app.deleteSessionMessage"),
      action: async () => {
        try {
          await invoke("delete_session", { agentName, sessionId, archived });
          if (!archived) finishSessionMutation(agentName, sessionId);
          await refreshSessions([agentName]);
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
    restoreSession,
    deleteSession,
  };
}
