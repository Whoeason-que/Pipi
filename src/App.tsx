import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import AgentWorkbench from "./agents/AgentWorkbench";
import CreateAgentForm from "./agents/CreateAgentForm";
import ChatView from "./Chat";
import {
  formatRuntimeError,
  normalizeAgentEvent
} from "./chat-runtime";
import ConfirmModal from "./components/ConfirmModal";
import EmptyState from "./components/EmptyState";
import {
  IconAgent,
  IconArchive,
  IconClose,
  IconGear,
  IconMenu,
  IconPlus,
  IconRestore,
  IconSearch,
  IconSubagent,
  IconTrash
} from "./icons";
import Login from "./Login";
import {
  getAuthStatus,
  getConnectionState,
  getRuntimeEndpoint,
  invoke,
  isTauriRuntime,
  listen,
  onAuthRequired,
  subscribeConnection,
  type ConnectionState
} from "./platform";
import { useResizableWidth } from "./resizable";
import { SearchPanel } from "./SearchPanel";
import { SessionRuntime } from "./session-runtime";
import SettingsView from "./settings/SettingsView";
import { applyTheme } from "./theme";
import {
  type AgentDefinition,
  type SessionInfoView,
  type SessionSummaryView,
  type Settings
} from "./types";

const CONNECTION_LABELS: Record<ConnectionState, string> = {
  online: "已连接",
  connecting: "连接中",
  offline: "已断开",
  dev: "演示模式",
};

/**
 * 左侧图标栏的分区：Agent / Subagent / 搜索各自一个侧栏面板，
 * 全局设置（settingsOpen）是主区里的一个视图，不在分区状态里。
 */
type NavSection = "agents" | "subs" | "search";

/** 构建时注入的版本号（vite define），未注入时留空。 */
const APP_VERSION = typeof __PIPI_VERSION__ === "string" ? __PIPI_VERSION__ : "";

/** 状态栏的运行位置描述由 platform 统一提供（避免两处各自推导服务地址）。 */

export default function App() {
  const [sessionRuntime] = useState(() => new SessionRuntime({ invoke, listen }));
  const [authStatus, setAuthStatus] = useState<{
    authRequired: boolean;
    authenticated: boolean;
    checking: boolean;
  }>({
    authRequired: false,
    authenticated: true,
    checking: !isTauriRuntime(),
  });
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [archivedAgents, setArchivedAgents] = useState<AgentDefinition[]>([]);
  const [archivedOpen, setArchivedOpen] = useState(true);
  const [archivedSessions, setArchivedSessions] = useState<
    Record<string, SessionSummaryView[]>
  >({});
  const [archivedSessionsExpanded, setArchivedSessionsExpanded] = useState<
    Record<string, boolean>
  >({});
  /** 左栏分区（Agent / Subagent / 搜索）；设置是主区视图，由 settingsOpen 控制。 */
  const [navSection, setNavSection] = useState<NavSection>("agents");
  /** 侧栏里哪些 Agent 的会话列表展开了（折叠后只显示 agent-row）。 */
  const [expandedAgents, setExpandedAgents] = useState<Record<string, boolean>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [chatOpen, setChatOpen] = useState(false);
  const [chatKey, setChatKey] = useState(0);
  const [sessionsByAgent, setSessionsByAgent] = useState<Record<string, SessionSummaryView[]>>({});
  const [activeSession, setActiveSession] = useState<SessionInfoView | null>(null);
  /**
   * 正在跑一轮的会话集合，键是 `conversationKey(Agent, 会话 id)`。
   * 多会话并发下运行态是**集合**：同一个 Agent 可以有多条会话在跑、不同 Agent 也可以，
   * 每条各自一个运行守卫。这个集合只用于显示（侧栏标记 / 底部计数）——守卫在核心。
   */
  const [runningSessions, setRunningSessions] = useState<Set<string>>(() => new Set());
  /** 当前查看的会话 id（null = 新会话，还没落文件）。 */
  const [viewSessionId, setViewSessionId] = useState<string | null>(null);
  const runningKey = useCallback(
    (agentName: string, sessionId: string) => `${agentName}\u0000${sessionId}`,
    [],
  );
  // 身份稳定：搜索面板按它做 memo，否则每次渲染都会重算全部会话
  const isConversationRunning = useCallback(
    (agentName: string, sessionId: string | null) =>
      sessionId !== null && runningSessions.has(runningKey(agentName, sessionId)),
    [runningSessions, runningKey],
  );
  const agentHasRunning = (agentName: string) =>
    Array.from(runningSessions).some((key) => key.startsWith(`${agentName}\u0000`));
  // 侧栏宽度可拖拽调节（窄屏抽屉模式下由媒体查询接管，见 responsive.css）
  const sidebarResize = useResizableWidth({
    storageKey: "sidebar-width",
    initial: 260,
    min: 200,
    max: 460,
    edge: "right",
  });
  const sidebarWidth = sidebarResize.width;
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [connection, setConnection] = useState<ConnectionState>(getConnectionState());
  const [error, setError] = useState<string | null>(null);
  // 删除确认弹窗（window.confirm 在 Tauri WebView 不可用，一律走自定义弹层）
  const [confirmState, setConfirmState] = useState<{
    title: string;
    message: string;
    action: () => Promise<void>;
  } | null>(null);
  const agentsRef = useRef<AgentDefinition[]>([]);
  const sessionListRequestRef = useRef(new Map<string, number>());
  const agentRequestRef = useRef(0);
  const navigationRequestRef = useRef(0);
  const sessionInfoRequestRef = useRef(0);
  const settingsSaveRequestRef = useRef(0);
  const settingsRef = useRef<Settings | null>(null);
  const settingsSaveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const selectedRef = useRef<string | null>(null);
  const activeSessionRef = useRef<SessionInfoView | null>(null);
  const blockedSessionIdsRef = useRef(new Set<string>());

  const navigationQueueRef = useRef<Promise<void>>(Promise.resolve());
  agentsRef.current = agents;
  settingsRef.current = settings;
  selectedRef.current = selected;
  activeSessionRef.current = activeSession;

  useEffect(() => subscribeConnection(setConnection), []);

  const safeSetError = useCallback((msg: string | null) => {
    if (!msg) {
      setError(null);
      return;
    }
    if (msg.includes("需要 PIPI_AUTH_TOKEN")) return;
    // 核心的「运行中」拒绝：前端状态可能滞后于核心（例如界面刚重载），
    // 换成可执行的指引而不是把核心原文丢给用户。
    if (msg.includes("当前会话仍在运行")) {
      setError("Agent 正在运行：请进入该 Agent 的会话停止，或等它完成后再操作");
      return;
    }
    if (msg.includes("正在运行或有托管后台任务")) {
      setError("会话仍在运行或有后台任务：请先停止运行并终止后台任务后再操作");
      return;
    }
    setError(msg);
  }, []);

  /** 立刻向核心核对一次运行态（新会话拿到 id 前 / 事件驱动变化时用）。 */
  const syncRunningRef = useRef<() => void>(() => {});
  const runningRequestRef = useRef(0);

  // 身份必须保持稳定（ChatView 的 effect 以它为依赖）：运行态变化按会话合并。
  const handleRunningChange = useCallback(() => { syncRunningRef.current(); }, []);

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

  useEffect(() => {
    let active = true;
    if (!isTauriRuntime()) {
      void getAuthStatus().then((status) => {
        if (!active) return;
        setAuthStatus({
          authRequired: status.authRequired,
          authenticated: status.authenticated,
          checking: false,
        });
      });
    }
    const unsub = onAuthRequired(() => {
      setAuthStatus((prev) => ({ ...prev, authenticated: false }));
    });
    return () => {
      active = false;
      unsub();
    };
  }, []);

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

  const refreshSessions = useCallback(async (names: string[]) => {
    const results = await Promise.all(names.map(async name => {
      const requestId = (sessionListRequestRef.current.get(name) ?? 0) + 1;
      sessionListRequestRef.current.set(name, requestId);
      try {
        const sessions = await invoke("list_sessions", { agentName: name });
        return { name, requestId, sessions, failed: false };
      } catch {
        return { name, requestId, sessions: null, failed: true };
      }
    }));
    setSessionsByAgent(previous => {
      const known = new Set(agentsRef.current.map(agent => agent.name));
      const next = Object.fromEntries(Object.entries(previous).filter(([name]) => known.has(name)));
      for (const result of results) {
        if (known.has(result.name) && result.requestId === sessionListRequestRef.current.get(result.name)
          && result.sessions) next[result.name] = result.sessions;
      }
      return next;
    });
    const failedNames = results.filter(result => result.failed
      && result.requestId === sessionListRequestRef.current.get(result.name)).map(result => result.name);
    if (failedNames.length) safeSetError(`会话列表加载失败：${failedNames.join("、")}（保留旧数据）`);
  }, [safeSetError]);

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
        if (
          errorValue
          && typeof errorValue === "object"
          && (errorValue as { isAuthError?: boolean }).isAuthError
        ) {
          return;
        }
        const formatted = formatRuntimeError(errorValue);
        if (formatted !== "需要 PIPI_AUTH_TOKEN") safeSetError(formatted);
      }
    }
  }, [safeSetError]);

  const refreshArchived = useCallback(async () => {
    try {
      const next = await invoke("list_archived_agents");
      setArchivedAgents(next);
    } catch (errorValue) {
      const formatted = formatRuntimeError(errorValue);
      if (formatted !== "需要 PIPI_AUTH_TOKEN") safeSetError(formatted);
    }
  }, [safeSetError]);

  useEffect(() => {
    if (authStatus.checking || (authStatus.authRequired && !authStatus.authenticated)) {
      return;
    }
    void refresh();
    void refreshArchived();
    let active = true;
    invoke("get_settings")
      .then((nextSettings) => {
        if (!active) return;
        setSettings(nextSettings);
        applyTheme(nextSettings.theme);
      })
      .catch((errorValue) => {
        if (!active) return;
        if (
          errorValue
          && typeof errorValue === "object"
          && (errorValue as { isAuthError?: boolean }).isAuthError
        ) {
          return;
        }
        const formatted = formatRuntimeError(errorValue);
        if (formatted !== "需要 PIPI_AUTH_TOKEN") safeSetError(formatted);
      });
    return () => {
      active = false;
    };
  }, [authStatus.checking, authStatus.authRequired, authStatus.authenticated, refresh, refreshArchived, safeSetError]);

  // agents 变化后拉取各 Agent 的会话列表；空列表也要清理旧数据。
  useEffect(() => {
    if (agents.length === 0) {
      setSessionsByAgent({});
      return;
    }
    void refreshSessions(agents.map((agent) => agent.name));
  }, [agents, refreshSessions]);

  useEffect(() => {
    if (!sidebarOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSidebarOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [sidebarOpen]);

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
      const event = { payload: frame.payload };
      const normalized = normalizeAgentEvent(event.payload);
      if (normalized.event.type === "agent_start" || normalized.event.type === "agent_end") syncRunningRef.current();
      if (normalized.event.type !== "agent_end") return;
      if (normalized.meta) void refreshSessions([normalized.meta.agentName]);
      if (
        normalized.meta
        && (blockedSessionIdsRef.current.has(normalized.meta.sessionId)
          || normalized.meta.agentName !== selectedRef.current
          || (activeSessionRef.current?.sessionId
            && normalized.meta.sessionId !== activeSessionRef.current.sessionId)
          // 已结算的旧 run 的迟到结束事件不再触发刷新
          || (activeSessionRef.current?.runId != null
            && normalized.meta.runId < activeSessionRef.current.runId))
      ) return;
      const targetAgent = normalized.meta?.agentName;
      const targetSession = normalized.meta?.sessionId;
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
  }, [sessionRuntime, refreshSessions, safeSetError]);

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
      let infoRequestId = 0;
      try {
        await invoke("open_session", { agentName, sessionId });
        if (requestId !== navigationRequestRef.current) {
          return;
        }
        setSelected(agentName);
        setCreating(false);
        setSettingsOpen(false);
        setChatOpen(true);
        setSidebarOpen(false);
        setExpandedAgents((prev) => ({ ...prev, [agentName]: true }));
        setViewSessionId(sessionId);
        setActiveSession(null);
        setChatKey((key) => key + 1);
        infoRequestId = ++sessionInfoRequestRef.current;
        const info = await invoke("session_info", { agentName, sessionId });
        if (
          requestId === navigationRequestRef.current
          && infoRequestId === sessionInfoRequestRef.current
        ) {
          setActiveSession(info);
        }
      } catch (errorValue) {
        if (
          requestId === navigationRequestRef.current
          && (infoRequestId === 0 || infoRequestId === sessionInfoRequestRef.current)
        ) {
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
        setSelected(agentName);
        setCreating(false);
        setSettingsOpen(false);
        setChatOpen(true);
        setSidebarOpen(false);
        setExpandedAgents((prev) => ({ ...prev, [agentName]: true }));
        setViewSessionId(null);
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
    setViewSessionId(null);
    setActiveSession(null);
    setChatKey((key) => key + 1);
    return true;
  };

  const updateSettings = (next: Settings): Promise<void> => {
    const previous = settingsRef.current;
    const requestId = ++settingsSaveRequestRef.current;
    settingsRef.current = next;
    setSettings(next);
    applyTheme(next.theme);

    const save = settingsSaveQueueRef.current.then(async () => {
      try {
        await invoke("save_settings", { settings: next });
      } catch (errorValue) {
        if (requestId === settingsSaveRequestRef.current) {
          settingsRef.current = previous;
          setSettings(previous);
          if (previous) applyTheme(previous.theme);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
    settingsSaveQueueRef.current = save.catch(() => {});
    return save;
  };

  const current = agents.find((a) => a.name === selected) ?? null;

  // ============ 归档 / 恢复 / 删除 ============
  // 归档 = 移到 .archive/（文件即真相）；删除不可恢复，一律 confirm 后才执行。

  /**
   * 归档/删除当前正在查看的会话后，留在 ChatView 里进入一条干净的新会话。
   * 核心会在文件操作前释放空闲会话槽；这里同步清理前端身份与事件回放缓存，
   * 避免界面继续把已归档/已删除的会话当成可发送目标。
   */
  const finishSessionMutation = (agentName: string, sessionId: string) => {
    sessionRuntime.forget(agentName, sessionId);
    setRunningSessions((previous) => {
      const key = runningKey(agentName, sessionId);
      if (!previous.has(key)) return previous;
      const next = new Set(previous);
      next.delete(key);
      return next;
    });
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
    const prefix = `${agentName}\u0000`;
    sessionRuntime.forgetAgent(agentName);
    setRunningSessions((previous) => {
      const next = new Set([...previous].filter((key) => !key.startsWith(prefix)));
      return next.size === previous.size ? previous : next;
    });
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
      setSessionsByAgent((previous) => {
        const next = { ...previous };
        delete next[name];
        return next;
      });
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
          await invoke(archived ? "delete_archived_agent" : "delete_agent", { name });
          if (!archived) forgetAgentRuntime(name);
          if (!archived && selectedRef.current === name) {
            setSelected(null);
            setChatOpen(false);
          }
          if (!archived) {
            setSessionsByAgent((previous) => {
              const next = { ...previous };
              delete next[name];
              return next;
            });
          }
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
        const list = await invoke("list_archived_sessions", {
          agentName,
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
      const list = await invoke("list_archived_sessions", {
        agentName,
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
          await invoke("delete_archived_session", { agentName, sessionId });
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

  const selectAgent = (agentName: string) => {
    // 切换不被任何运行态拦住：每条会话是独立的一路，切走不影响它继续跑
    //（ChatView 卸载也不中止）。这里只做「选中 + 收起会话视图」。
    const previous = selectedRef.current;
    invalidateNavigation();
    // 释放上一个 Agent 名下**空闲**的会话（不落盘，文件还在），否则它们会一直
    // 被占用检查锁住（归档 / 删除被拒）。正在跑的那条留着 —— 后台继续跑。
    if (previous && previous !== agentName) {
      void invoke("new_session", { agentName: previous }).catch(() => {});
    }
    setSelected(agentName);
    setCreating(false);
    setChatOpen(false);
    setSettingsOpen(false);
    setSidebarOpen(false);
  };

  const startCreating = () => {
    invalidateNavigation();
    setCreating(true);
    setSettingsOpen(false);
    setSidebarOpen(false);
  };

  /// 左栏分区切换：设置是主区的另一个视图，切分区即离开设置。
  /// 顺带 setSidebarOpen(true)：宽屏无副作用，窄屏下等于展开抽屉。
  const openSection = (section: NavSection) => {
    setNavSection(section);
    setSettingsOpen(false);
    setSidebarOpen(true);
  };

  const sessionsFor = useCallback((agent: AgentDefinition): SessionSummaryView[] =>
    sessionsByAgent[agent.name] ?? [], [sessionsByAgent]);

  const regularAgents = agents.filter((agent) => !agent.subagent);
  const subagents = agents.filter((agent) => agent.subagent);
  const sectionAgents = navSection === "subs" ? subagents : regularAgents;
  const sectionArchived = archivedAgents.filter(
    (agent) => agent.subagent === (navSection === "subs"),
  );

  const sessionTotal = useMemo(
    () => Object.values(sessionsByAgent).reduce((total, list) => total + list.length, 0),
    [sessionsByAgent],
  );

  if (authStatus.checking) {
    return <div className="login-container" />;
  }

  if (authStatus.authRequired && !authStatus.authenticated) {
    return (
      <Login
        onSuccess={() => {
          setAuthStatus((prev) => ({ ...prev, authenticated: true }));
          safeSetError(null);
        }}
      />
    );
  }

  return (
    <div
      className={`app${sidebarOpen ? " sidebar-open" : ""}`}
      style={{ "--sidebar-width": `${sidebarWidth}px` } as React.CSSProperties}
    >
      <div className="app-body">
        <nav className="rail" aria-label="主导航">
          <button
            type="button"
            className={`icon-btn${!settingsOpen && navSection === "agents" ? " active" : ""}`}
            title="Agent 列表"
            aria-label="Agent 列表"
            aria-current={!settingsOpen && navSection === "agents" ? "page" : undefined}
            onClick={() => openSection("agents")}
          >
            <IconAgent />
          </button>
          <button
            type="button"
            className={`icon-btn${!settingsOpen && navSection === "subs" ? " active" : ""}`}
            title="Subagent 列表"
            aria-label="Subagent 列表"
            aria-current={!settingsOpen && navSection === "subs" ? "page" : undefined}
            onClick={() => openSection("subs")}
          >
            <IconSubagent />
          </button>
          <button
            type="button"
            className={`icon-btn${!settingsOpen && navSection === "search" ? " active" : ""}`}
            title="搜索"
            aria-label="搜索"
            aria-current={!settingsOpen && navSection === "search" ? "page" : undefined}
            onClick={() => openSection("search")}
          >
            <IconSearch />
          </button>
          <span className="rail-spacer" />
          <button
            type="button"
            className={`icon-btn${settingsOpen ? " active" : ""}`}
            title={settingsOpen ? "关闭设置" : "设置"}
            aria-label={settingsOpen ? "关闭设置" : "打开设置"}
            aria-current={settingsOpen ? "page" : undefined}
            onClick={() => {
              setSidebarOpen(false);
              // 再点一次即返回设置前的界面（与返回按钮同义）
              setSettingsOpen((open) => !open);
            }}
          >
            <IconGear />
          </button>
        </nav>

        <aside className="sidebar" id="primary-navigation" aria-label="Agent 与会话列表">
          <div className="sb-head">
            <span className="wordmark">Pipi</span>
            {APP_VERSION && <span className="ver-chip">v{APP_VERSION}</span>}
          </div>

          {navSection === "search" ? (
            <SearchPanel
              query={searchQuery}
              onQueryChange={setSearchQuery}
              agents={agents}
              sessionsByAgent={sessionsByAgent}
              isRunning={isConversationRunning}
              onSelectAgent={(name) => {
                selectAgent(name);
                setExpandedAgents((previous) => ({ ...previous, [name]: true }));
              }}
              onOpenSession={(agentName, sessionId) => void openSession(agentName, sessionId)}
            />
          ) : (
            <>
              <div className="sb-sec agent-group-heading">
                <span>{navSection === "subs" ? "SUBS" : "AGENTS"}</span>
                <span className="spacer" />
                <span className="sb-count">{sectionAgents.length}</span>
                {navSection === "agents" && (
                  <button
                    type="button"
                    className="icon-btn add"
                    title="新建 Agent"
                    aria-label="新建 Agent"
                    onClick={startCreating}
                  >
                    <IconPlus />
                  </button>
                )}
              </div>

              <nav
                className="agents"
                aria-label={navSection === "subs" ? "Subagent 列表" : "Agent 列表"}
              >
                {sectionAgents.map((a) => {
                  const isActiveAgent = selected === a.name && !creating;
                  const sessions = sessionsFor(a);
                  const sessionCount = sessionsByAgent[a.name]?.length ?? 0;
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
                          onClick={() => selectAgent(a.name)}
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
                              selectAgent(a.name);
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
                              void startNewSession(a.name);
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
                              void archiveAgent(a.name);
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
                              void deleteAgent(a.name, false);
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
                              setExpandedAgents((prev) => ({
                                ...prev,
                                [a.name]: !isExpanded,
                              }));
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
                                      onClick={() => void openSession(a.name, sess.id)}
                                      onKeyDown={(event) => {
                                        if (event.target !== event.currentTarget) return;
                                        if (event.key === "Enter" || event.key === " ") {
                                          event.preventDefault();
                                          void openSession(a.name, sess.id);
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
                                              void archiveSession(a.name, sess.id);
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
                                              void deleteSession(a.name, sess.id);
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
                                  onClick={() => void toggleArchivedSessions(a.name)}
                                  onKeyDown={(event) => {
                                    if (event.key === "Enter" || event.key === " ") {
                                      event.preventDefault();
                                      void toggleArchivedSessions(a.name);
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
                                                  void restoreSession(a.name, sess.id);
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
                                                  void deleteArchivedSession(a.name, sess.id);
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
                {sectionAgents.length === 0 && (
                  <div className="session pending">
                    {navSection === "subs"
                      ? "没有 Subagent：由 Agent 的 create_agent 工具创建后归入这里"
                      : "还没有 Agent：点上方 + 新建"}
                  </div>
                )}
              </nav>

              {sectionArchived.length > 0 && (
                <>
                  <div
                    className="sb-sec archived-head"
                    role="button"
                    tabIndex={0}
                    aria-expanded={archivedOpen}
                    onClick={() => setArchivedOpen((open) => !open)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        setArchivedOpen((open) => !open);
                      }
                    }}
                  >
                    <span>已归档</span>
                    <span className="spacer" />
                    <span className="sb-count">{sectionArchived.length}</span>
                    <span className={`caret${archivedOpen ? " open" : ""}`}>▶</span>
                  </div>
                  {archivedOpen && (
                    <div className="archived">
                      {sectionArchived.map((agent) => (
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
                                void restoreAgent(agent.name);
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
                                void deleteAgent(agent.name, true);
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
          )}
        </aside>

        <div
          className="resize-handle sidebar-resize"
          title="拖动调整侧栏宽度（←/→ 微调）"
          aria-label="调整侧栏宽度"
          {...sidebarResize.handleProps}
        />

        {sidebarOpen && (
          <button
            type="button"
            className="sidebar-backdrop"
            aria-label="关闭导航"
            onClick={() => setSidebarOpen(false)}
          />
        )}

        <main className="main">
          <div className="mobile-toolbar">
            <button
              type="button"
              className="icon-btn"
              aria-label="打开导航"
              title="打开导航"
              aria-expanded={sidebarOpen}
              aria-controls="primary-navigation"
              onClick={() => setSidebarOpen(true)}
            >
              <IconMenu />
            </button>
            <span className="mobile-toolbar-title">
              {settingsOpen ? "设置" : creating ? "新建 Agent" : current?.name ?? "Pipi"}
            </span>
          </div>

          {error && (
            <div className="error" role="alert">
              <span>{error}</span>
              <button
                type="button"
                className="error-dismiss"
                onClick={() => setError(null)}
                aria-label="关闭错误提示"
              >
                <IconClose />
              </button>
            </div>
          )}

          {settingsOpen && settings ? (
            <SettingsView
              settings={settings}
              onChange={updateSettings}
              onClose={() => setSettingsOpen(false)}
              showLogout={!isTauriRuntime() && authStatus.authRequired}
            />
          ) : creating ? (
            <CreateAgentForm
              providers={settings?.providers ?? []}
              defaultProviderId={settings?.defaultProviderId ?? null}
              onCreated={async (name) => {
                setCreating(false);
                await refresh();
                setSelected(name);
              }}
              onCancel={() => setCreating(false)}
              onError={safeSetError}
            />
          ) : current ? (
            settings && (
              chatOpen ? (
                <ChatView
                  key={`${current.name}-${chatKey}`}
                  agent={current}
                  providers={settings.providers}
                  sessionId={viewSessionId}
                  sessions={sessionRuntime}
                  blockedSessionIds={[...blockedSessionIdsRef.current]}
                  onBack={() => {
                    invalidateNavigation();
                    // 释放本 Agent 空闲的会话（否则归档/删除会被占用检查拒），
                    // 正在跑的那条留着：返回后它继续跑（卸载不再中止运行）。
                    void invoke("new_session", { agentName: current.name }).catch(() => {});
                    setChatOpen(false);
                    setSidebarOpen(false);
                  }}
                  onError={safeSetError}
                  onNewSession={createSessionFromChat}
                  onRunningChange={handleRunningChange}
                  onSessionReset={(info) => {
                    setActiveSession(info ?? null);
                    if (info) setViewSessionId(info.sessionId);
                    void refreshSessions([current.name]);
                    // 压缩换会话可能把原会话移进归档：折叠区正展开时同步刷新
                    if (archivedSessionsExpanded[current.name]) {
                      void invoke("list_archived_sessions", {
                        agentName: current.name,
                      })
                        .then((list) =>
                          setArchivedSessions((previous) => ({
                            ...previous,
                            [current.name]: list,
                          })),
                        )
                        .catch(() => {});
                    }
                  }}
                />
              ) : (
                <AgentWorkbench
                  key={current.name}
                  agent={current}
                  providers={settings.providers}
                  onSaved={async (subagent) => {
                    if (subagent !== undefined) {
                      // Agent 换了分组：切到它现在所属的分区，不然会在当前列表里"消失"
                      setNavSection(subagent ? "subs" : "agents");
                    }
                    await refresh();
                  }}
                  onBack={() => setSelected(null)}
                  onError={safeSetError}
                  blockedSessionIds={[...blockedSessionIdsRef.current]}
                  sessions={sessionRuntime}
                  onRunningChange={handleRunningChange}
                />
              )
            )
          ) : (
            <EmptyState hasAgents={agents.length > 0} />
          )}
        </main>
      </div>

      <footer className="status-bar" aria-label="运行状态">
        <span className="conn">
          <i className={`status-dot ${connection}`} aria-hidden="true" />
          {CONNECTION_LABELS[connection]}
        </span>
        <span className="sep">│</span>
        <span className="opt">{getRuntimeEndpoint()}</span>
        <span className="sep opt">│</span>
        <span className="opt">
          数据根目录 <b>~/.pipi/agents</b>
        </span>
        <span className="sep">│</span>
        <span>
          <b>{agents.length}</b> 个 Agent · <b>{sessionTotal}</b> 个会话
        </span>
        <span className="spacer" />
        <span>
          运行中 <b>{runningSessions.size}</b>
        </span>
        {APP_VERSION && (
          <>
            <span className="sep">│</span>
            <span>
              Pipi <b>v{APP_VERSION}</b>
            </span>
          </>
        )}
      </footer>

      {confirmState && (
        <ConfirmModal
          title={confirmState.title}
          message={confirmState.message}
          onConfirm={confirmState.action}
          onCancel={() => setConfirmState(null)}
        />
      )}
    </div>
  );
}
