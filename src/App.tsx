import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  invoke,
  listen,
  isTauriRuntime,
  getAuthStatus,
  getConnectionState,
  getRuntimeEndpoint,
  onAuthRequired,
  logout,
  subscribeConnection,
  type ConnectionState,
} from "./platform";
import ChatView, { SessionDiagnostics } from "./Chat";
import Login from "./Login";
import { useResizableWidth } from "./resizable";
import {
  IconArchive,
  IconBack,
  IconClose,
  IconGear,
  IconMenu,
  IconPlus,
  IconRestore,
  IconSearch,
  IconTrash,
} from "./icons";
import {
  catalogSourceLabel,
  providerGroups,
  providerSeed,
  resolveContextWindow,
  resolveMaxTokens,
  type CatalogModel,
  type CatalogProvider,
  type ModelCatalog,
} from "./catalog";
import { loadCatalog, peekCatalog, resetCatalog } from "./catalog-client";
import { ModelPicker } from "./ModelPicker";
import { ChoiceSelect } from "./Select";
import {
  formatRetryDelay,
  formatRuntimeError,
  normalizeAgentEvent,
  type AgentEventPayload,
} from "./chat-runtime";
import {
  API_LABELS,
  SANDBOX_LABELS,
  type AgentDefinition,
  type SessionInfoView,
  type SessionSummaryView,
  type Theme,
  type ApiKind,
  type BashMode,
  type PermissionsConfig,
  type ProviderConfig,
  type SandboxMode,
  type Settings,
} from "./types";

const BASH_MODE_LABELS: Record<BashMode, string> = {
  allowAll: "全部允许",
  allowlist: "白名单",
  denylist: "黑名单",
};

const CONNECTION_LABELS: Record<ConnectionState, string> = {
  online: "已连接",
  connecting: "连接中",
  offline: "已断开",
  dev: "演示模式",
};

const DEFAULT_TOOLS = ["read", "write", "edit", "bash", "memory", "glob", "grep"] as const;
const KNOWN_TOOLS = [
  ...DEFAULT_TOOLS,
  "create_agent",
  "run_agent",
  "read_agent",
] as const;

/** 构建时注入的版本号（vite define），未注入时留空。 */
const APP_VERSION = typeof __PIPI_VERSION__ === "string" ? __PIPI_VERSION__ : "";

function applyTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("pipi-theme", theme);
}

function keyStatus(p: ProviderConfig): { label: string; warn: boolean } {
  if (p.envKey) return { label: `env: ${p.envKey}`, warn: false };
  if (p.apiKey) return { label: "已存密钥", warn: false };
  return { label: "未配置密钥", warn: true };
}

/** 状态栏的运行位置描述由 platform 统一提供（避免两处各自推导服务地址）。 */

export default function App() {
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
  const runningKey = (agentName: string, sessionId: string) => `${agentName}\u0000${sessionId}`;
  const isConversationRunning = (agentName: string, sessionId: string | null) =>
    sessionId !== null && runningSessions.has(runningKey(agentName, sessionId));
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
  const [filter, setFilter] = useState("");
  const [connection, setConnection] = useState<ConnectionState>(getConnectionState());
  const [error, setError] = useState<string | null>(null);
  // 删除确认弹窗（window.confirm 在 Tauri WebView 不可用，一律走自定义弹层）
  const [confirmState, setConfirmState] = useState<{
    title: string;
    message: string;
    action: () => Promise<void>;
  } | null>(null);
  const agentsRef = useRef<AgentDefinition[]>([]);
  const sessionListRequestRef = useRef(0);
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
    setError(msg);
  }, []);

  /** 立刻向核心核对一次运行态（新会话拿到 id 前 / 事件驱动变化时用）。 */
  const syncRunningRef = useRef<() => void>(() => {});

  // 身份必须保持稳定（ChatView 的 effect 以它为依赖）：运行态变化按会话合并。
  const handleRunningChange = useCallback(
    (agentName: string, sessionId: string | null, running: boolean) => {
      if (sessionId === null) {
        // 新会话还没拿到 id：立刻向核心核对一次（轮询也会兜底）
        syncRunningRef.current();
        return;
      }
      setRunningSessions((previous) => {
        const next = new Set(previous);
        const key = `${agentName}\u0000${sessionId}`;
        if (running) next.add(key);
        else next.delete(key);
        return next;
      });
    },
    [],
  );

  // 运行态以核心为准：前端可能整体重载（HMR / 刷新）而核心里的运行还在继续，
  // 只靠 ChatView 的 onRunningChange 会让界面自认空闲、守卫放行后撞上核心拒绝。
  // 另外，视图关着时（切到了别的 Agent）运行结束的信号不会到达前端，所以只要
  // 还有 Agent 在跑就轮询兜底 —— 开销是一次本地 IPC。
  useEffect(() => {
    let active = true;
    const sync = () => {
      void invoke<SessionInfoView[]>("session_infos")
        .then((infos) => {
          if (!active) return;
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
    const requestId = ++sessionListRequestRef.current;
    const results = await Promise.all(
      names.map(async (name) => {
        try {
          return {
            name,
            sessions: await invoke<SessionSummaryView[]>("list_sessions", { agentName: name }),
            failed: false,
          } as const;
        } catch {
          return { name, sessions: null, failed: true } as const;
        }
      }),
    );
    if (requestId !== sessionListRequestRef.current) return;
    setSessionsByAgent((previous) => {
      const next: Record<string, SessionSummaryView[]> = {};
      for (const result of results) {
        if (!result.failed && result.sessions) next[result.name] = result.sessions;
        else if (previous[result.name]) next[result.name] = previous[result.name];
      }
      return next;
    });
    const failedNames = results.filter((result) => result.failed).map((result) => result.name);
    if (failedNames.length > 0) {
      safeSetError(`会话列表加载失败：${failedNames.join("、")}（保留旧数据）`);
    }
  }, [safeSetError]);

  const refresh = useCallback(async () => {
    const requestId = ++agentRequestRef.current;
    try {
      const nextAgents = await invoke<AgentDefinition[]>("list_agents");
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
      const next = await invoke<AgentDefinition[]>("list_archived_agents");
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
    invoke<Settings>("get_settings")
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
    const unlisten = listen<AgentEventPayload>("agent-event", (event) => {
      if (!active) return;
      const normalized = normalizeAgentEvent(event.payload);
      if (normalized.event.type !== "agent_end") return;
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
        void invoke<SessionInfoView | null>("session_info", {
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
      void refreshSessions(agentsRef.current.map((agent) => agent.name));
    });
    return () => {
      active = false;
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [refreshSessions]);

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
        setViewSessionId(sessionId);
        setActiveSession(null);
        setChatKey((key) => key + 1);
        infoRequestId = ++sessionInfoRequestRef.current;
        const info = await invoke<SessionInfoView | null>("session_info", { agentName, sessionId });
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

  const archiveAgent = async (name: string) => {
    try {
      await invoke("archive_agent", { name });
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
      await refreshSessions([agentName]);
      // 「已归档」折叠区若正展开，同步回写新归档的会话，避免收起再展开才可见
      if (archivedSessionsExpanded[agentName]) {
        const list = await invoke<SessionSummaryView[]>("list_archived_sessions", {
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
      const list = await invoke<SessionSummaryView[]>("list_archived_sessions", {
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

  const query = filter.trim().toLowerCase();

  const sessionsFor = useCallback((agent: AgentDefinition): SessionSummaryView[] => {
    const list = sessionsByAgent[agent.name] ?? [];
    if (!query) return list;
    const agentHit = agent.name.toLowerCase().includes(query)
      || agent.description.toLowerCase().includes(query);
    return agentHit ? list : list.filter((s) => s.title.toLowerCase().includes(query));
  }, [query, sessionsByAgent]);

  const visibleAgents = useMemo(() => {
    if (!query) return agents;
    return agents.filter((agent) => (
      agent.name.toLowerCase().includes(query)
      || agent.description.toLowerCase().includes(query)
      || (sessionsByAgent[agent.name] ?? []).some((s) => s.title.toLowerCase().includes(query))
    ));
  }, [agents, query, sessionsByAgent]);

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
        <aside className="sidebar" id="primary-navigation" aria-label="主导航">
          <div className="sb-head">
            <span className="wordmark">Pipi</span>
            {APP_VERSION && <span className="ver-chip">v{APP_VERSION}</span>}
            <span className="spacer" />
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
          </div>

          <div className="sb-filter">
            <div className="search">
              <IconSearch />
              <input
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
                placeholder="筛选 Agents / 会话…"
                aria-label="筛选 Agents 与会话"
                autoComplete="off"
                spellCheck={false}
              />
            </div>
          </div>

          <div className="sb-sec">
            <span>Agents</span>
            <span className="spacer" />
            <span className="sb-count">{agents.length}</span>
            <button
              type="button"
              className="icon-btn add"
              title="新建 Agent"
              aria-label="新建 Agent"
              onClick={startCreating}
            >
              <IconPlus />
            </button>
          </div>

          <nav className="agents">
            {visibleAgents.map((a) => {
              const isActiveAgent = selected === a.name && !creating;
              const sessions = sessionsFor(a);

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
                    <span className="agent-count">{sessionsByAgent[a.name]?.length ?? 0}</span>
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
                  </div>
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
                            // 内层按钮（归档/删除）的键盘事件不冒泡成「打开会话」
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
                              <span className="session-title dim">{sess.title}</span>
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
                          ))
                        )}
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
            {agents.length > 0 && visibleAgents.length === 0 && (
              <div className="session pending">没有匹配的 Agent 或会话</div>
            )}
          </nav>

          {archivedAgents.length > 0 && (
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
                <span className="sb-count">{archivedAgents.length}</span>
                <span className={`caret${archivedOpen ? " open" : ""}`}>▶</span>
              </div>
              {archivedOpen && (
                <div className="archived">
                  {archivedAgents.map((agent) => (
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
            <CreateForm
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
                      void invoke<SessionSummaryView[]>("list_archived_sessions", {
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
                <AgentDetail
                  key={current.name}
                  agent={current}
                  providers={settings.providers}
                  onSaved={refresh}
                  onBack={() => setSelected(null)}
                  onError={safeSetError}
                  blockedSessionIds={[...blockedSessionIdsRef.current]}
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

function EmptyState({ hasAgents }: { hasAgents: boolean }) {
  return (
    <div className="empty">
      <div className="brand">Pipi</div>
      <h1>{hasAgents ? "选择一个 Agent" : "创建你的第一个 Agent"}</h1>
      <p>
        在 Pipi 里，你维护的不是一条条会话，而是一群有名字、有工作目录、
        有技能和记忆的 Agent。会话只是 Agent 的一次运行记录，是副产品。
      </p>
    </div>
  );
}

// ============ Agent 详情 ============

interface AgentDetailProps {
  agent: AgentDefinition;
  providers: ProviderConfig[];
  onSaved: () => void | Promise<void>;
  onBack: () => void;
  onError: (msg: string) => void;
  blockedSessionIds: string[];
  onRunningChange: (agentName: string, sessionId: string | null, running: boolean) => void;
}

type AgentSettingsTab = "basic" | "permissions" | "files" | "test";

function AgentDetail({
  agent,
  providers,
  onSaved,
  onBack,
  onError,
  blockedSessionIds,
  onRunningChange,
}: AgentDetailProps) {
  const { bash } = agent.permissions;
  const [testSession, setTestSession] = useState<SessionInfoView | null>(null);
  const [testLoadError, setTestLoadError] = useState<string | null>(null);
  const [testLoadVersion, setTestLoadVersion] = useState(0);
  const [settingsTab, setSettingsTab] = useState<AgentSettingsTab>("basic");
  const [providerId, setProviderId] = useState<string>(() => {
    const bound = providers.find(
      (p) => agent.provider && p.api === agent.provider.api && p.baseUrl === agent.provider.baseUrl,
    );
    return bound?.id ?? "";
  });
  const [modelId, setModelId] = useState(agent.provider?.id ?? "");
  // 从目录里选的模型：用它回填 maxTokens / contextWindow（不在目录里的模型保持原值）
  const [pickedModel, setPickedModel] = useState<CatalogModel | undefined>(undefined);
  // 当前「已保存绑定」的限额。换供应商后必须作废：否则手填目录外模型时会写回
  // 上一家供应商的旧限额（端点换成了 B、限额还是 A 的）。
  const [savedLimits, setSavedLimits] = useState(() => ({
    maxTokens: agent.provider?.maxTokens ?? 8192,
    contextWindow: agent.provider?.contextWindow ?? 0,
  }));
  const [saving, setSaving] = useState(false);

  // —— 元信息 / 权限编辑（agent.json 全字段）——
  const [description, setDescription] = useState(agent.description);
  const [workspace, setWorkspace] = useState(agent.workspace ?? "");
  const [tools, setTools] = useState<string[]>(agent.permissions.tools);
  const [bashMode, setBashMode] = useState<BashMode>(bash.mode);
  const [commands, setCommands] = useState(bash.commands.join("\n"));
  const [sandbox, setSandbox] = useState<SandboxMode>(agent.permissions.sandbox);
  // 自动压缩阈值（窗口占用的百分比，1–100）
  const [compactThreshold, setCompactThreshold] = useState(agent.compactThresholdPercent);
  useEffect(() => {
    let active = true;
    setTestSession(null);
    setTestLoadError(null);
    void invoke<SessionInfoView>("ensure_test_session", { agentName: agent.name })
      .then((info) => {
        if (!active) return;
        setTestSession(info);
      })
      .catch((errorValue) => {
        if (!active) return;
        const message = formatRuntimeError(errorValue);
        setTestLoadError(message);
        onError(message);
      });
    return () => {
      active = false;
    };
  }, [agent.name, onError, testLoadVersion]);

  const resetTestSession = async () => {
    const info = await invoke<SessionInfoView>("reset_test_session", { agentName: agent.name });
    setTestSession(info);
  };
  // agent 切换时重置编辑状态（否则上一个 Agent 的草稿会串台）
  const agentNameRef = useRef(agent.name);
  if (agentNameRef.current !== agent.name) {
    agentNameRef.current = agent.name;
    setDescription(agent.description);
    setWorkspace(agent.workspace ?? "");
    setTools(agent.permissions.tools);
    setCompactThreshold(agent.compactThresholdPercent);
    setBashMode(bash.mode);
    setCommands(bash.commands.join("\n"));
    setSandbox(agent.permissions.sandbox);
  }

  const selectedProvider = providers.find((provider) => provider.id === providerId);
  const draftProvider = selectedProvider
    ? {
        id: modelId.trim(),
        name: modelId.trim(),
        api: selectedProvider.api,
        baseUrl: selectedProvider.baseUrl,
        maxTokens: resolveMaxTokens(pickedModel, savedLimits.maxTokens),
        contextWindow: resolveContextWindow(pickedModel, savedLimits.contextWindow),
      }
    : null;
  const boundProviderId = providers.find(
    (provider) => agent.provider
      && provider.api === agent.provider.api
      && provider.baseUrl === agent.provider.baseUrl,
  )?.id ?? "";

  const providerDirty =
    providerId !== boundProviderId
    || modelId.trim() !== (agent.provider?.id ?? "")
    || (draftProvider?.maxTokens ?? 8192) !== (agent.provider?.maxTokens ?? 8192)
    || (draftProvider?.contextWindow ?? 0) !== (agent.provider?.contextWindow ?? 0);

  const saveConfiguration = async (running: boolean) => {
    if (saving || running || tools.length === 0) return;
    if (providerId && !modelId.trim()) {
      onError("请选择或填写默认模型后再保存");
      return;
    }
    setSaving(true);
    try {
      const next: AgentDefinition = {
        ...agent,
        model: modelId.trim(),
        provider: draftProvider,
        description: description.trim(),
        workspace: workspace.trim() || null,
        permissions: {
          tools,
          bash: {
            mode: bashMode,
            commands:
              bashMode === "allowAll"
                ? []
                : commands
                    .split("\n")
                    .map((c) => c.trim())
                    .filter(Boolean),
          },
          sandbox,
        },
        // 越界值在核心侧也会兜底，但先在这里夹一次，免得写进文件的是脏值
        compactThresholdPercent: Math.min(100, Math.max(1, Math.round(compactThreshold))),
      };
      await invoke("save_agent", { def: next });
      setSavedLimits({
        maxTokens: next.provider?.maxTokens ?? 8192,
        contextWindow: next.provider?.contextWindow ?? 0,
      });
      await onSaved();
      await resetTestSession();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSaving(false);
    }
  };

  const metaDirty =
    description !== agent.description ||
    (workspace.trim() || null) !== (agent.workspace ?? null) ||
    tools.join(",") !== agent.permissions.tools.join(",") ||
    bashMode !== bash.mode ||
    (bashMode === "allowAll" ? "" : commands) !== bash.commands.join("\n") ||
    sandbox !== agent.permissions.sandbox ||
    Math.min(100, Math.max(1, Math.round(compactThreshold))) !== agent.compactThresholdPercent;
  const configurationDirty = providerDirty || metaDirty;

  if (!testSession) {
    return (
      <div className="screen agent-workbench-loading">
        <div className="screen-bar">
          <button type="button" className="icon-btn" title="返回" aria-label="返回" onClick={onBack}>
            <IconBack />
          </button>
          <span className="crumb">Agent 工作台 · <b>{agent.name}</b></span>
        </div>
        <div className="empty compact-empty">
          <h2>{testLoadError ? "临时测试初始化失败" : "正在准备临时测试…"}</h2>
          {testLoadError && <p>{testLoadError}</p>}
          {testLoadError && (
            <button type="button" className="btn primary" onClick={() => setTestLoadVersion((value) => value + 1)}>
              重试
            </button>
          )}
        </div>
      </div>
    );
  }

  return (
    <ChatView
      key={`${agent.name}-${testSession.sessionId}`}
      agent={agent}
      providers={providers}
      sessionId={testSession.sessionId}
      blockedSessionIds={blockedSessionIds}
      onBack={onBack}
      onError={onError}
      onNewSession={async () => false}
      onRunningChange={onRunningChange}
      onSessionReset={(info) => {
        if (info?.temporary) setTestSession(info);
      }}
      mode="test"
      onClearTest={resetTestSession}
      onInspectDetail={() => setSettingsTab("test")}
      renderInspector={(context) => (
        <div className="agent-settings-panel">
          <div className="agent-settings-head">
            <div className="agent-settings-title">
              <span className="mono">{agent.name}</span>
              {configurationDirty && <span className="dirty-badge">未保存</span>}
            </div>
            <div className="sub">{agent.description || "（暂无描述）"}</div>
          </div>

          <div className="agent-settings-tabs" role="tablist" aria-label="Agent 设置">
            {([
              ["basic", "基础"],
              ["permissions", "权限"],
              ["files", "文件"],
              ["test", "测试"],
            ] as const).map(([tab, label]) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={settingsTab === tab}
                className={settingsTab === tab ? "active" : ""}
                onClick={() => setSettingsTab(tab)}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="agent-settings-body">
            {settingsTab === "basic" && (
              <section className="settings-section">
                <div className="settings-field">
                  <label htmlFor="agent-provider-select">provider</label>
                  <ChoiceSelect
                    id="agent-provider-select"
                    value={providerId}
                    choices={providers.map((provider) => ({ value: provider.id, label: provider.name }))}
                    placeholder="（未绑定提供商）"
                    isClearable
                    menuInPortal
                    onChange={(next) => {
                      setProviderId(next);
                      setModelId("");
                      setPickedModel(undefined);
                      setSavedLimits({ maxTokens: 8192, contextWindow: 0 });
                    }}
                  />
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-default-model">default_model</label>
                  <ModelPicker
                    id="agent-default-model"
                    providers={providers}
                    providerId={providerId}
                    modelId={modelId}
                    disabled={saving}
                    placeholder="模型 ID，如 claude-sonnet-4-5"
                    onModelChange={(nextId, model) => {
                      setModelId(nextId);
                      setPickedModel(model);
                    }}
                  />
                  <span className="hint">保存后，新的临时测试和正式会话会使用此模型。</span>
                  {draftProvider && (
                    <span className="hint mono">{draftProvider.api} · {draftProvider.baseUrl}</span>
                  )}
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-description">description</label>
                  <textarea
                    id="agent-description"
                    className="meta-editor"
                    value={description}
                    onChange={(event) => setDescription(event.target.value)}
                    placeholder="这个 Agent 是做什么的？"
                    rows={3}
                  />
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-workspace">workspace</label>
                  <input
                    id="agent-workspace"
                    className="mono"
                    value={workspace}
                    onChange={(event) => setWorkspace(event.target.value)}
                    placeholder={`~/.pipi/agents/${agent.name}/workspace（默认）`}
                  />
                </div>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="agent-compact-threshold">compact_threshold</label>
                  <div className="inline-value">
                    <input
                      id="agent-compact-threshold"
                      className="mono"
                      type="number"
                      min={1}
                      max={100}
                      value={compactThreshold}
                      onChange={(event) => setCompactThreshold(Number(event.target.value))}
                    />
                    <span>%</span>
                  </div>
                  <span className="hint">上下文占用达到模型窗口的该比例时自动压缩。</span>
                </div>
              </section>
            )}

            {settingsTab === "permissions" && (
              <section className="settings-section">
                <div className="settings-field">
                  <span className="field-label">tools</span>
                  <div className="tool-row settings-choice-grid">
                    {KNOWN_TOOLS.map((tool) => (
                      <label key={tool} className="tool-check">
                        <input
                          type="checkbox"
                          checked={tools.includes(tool)}
                          onChange={() => setTools((previous) => previous.includes(tool)
                            ? previous.filter((candidate) => candidate !== tool)
                            : [...previous, tool])}
                        />
                        <span className="mono">{tool}</span>
                      </label>
                    ))}
                  </div>
                  {tools.length === 0 && <span className="field-error">至少保留一个工具。</span>}
                </div>
                <div className="settings-field">
                  <span className="field-label">bash.mode</span>
                  <div className="tool-row vertical-choices">
                    {(Object.keys(BASH_MODE_LABELS) as BashMode[]).map((mode) => (
                      <label key={mode} className="tool-check">
                        <input
                          type="radio"
                          name="agent-bash-mode"
                          checked={bashMode === mode}
                          onChange={() => setBashMode(mode)}
                        />
                        <span>{BASH_MODE_LABELS[mode]}</span>
                      </label>
                    ))}
                  </div>
                  {bashMode !== "allowAll" && (
                    <div className="perm-editor">
                      <textarea
                        value={commands}
                        onChange={(event) => setCommands(event.target.value)}
                        placeholder={bashMode === "allowlist" ? "git\nnpm run\nls" : "rm\nsudo"}
                        rows={5}
                      />
                      <div className="hint">
                        {bashMode === "allowlist"
                          ? "每行一条；单词条目匹配命令名，带空格的条目按前缀匹配。"
                          : "每行一条；命中任意条目的命令将被拒绝。"}
                      </div>
                    </div>
                  )}
                </div>
                <div className="settings-field">
                  <span className="field-label">sandbox</span>
                  <div className="tool-row vertical-choices">
                    {(Object.keys(SANDBOX_LABELS) as SandboxMode[]).map((mode) => (
                      <label key={mode} className="tool-check">
                        <input
                          type="radio"
                          name="agent-sandbox"
                          checked={sandbox === mode}
                          onChange={() => setSandbox(mode)}
                        />
                        <span>{SANDBOX_LABELS[mode]}</span>
                      </label>
                    ))}
                  </div>
                  <span className="hint">
                    临时测试会按保存后的真实权限执行；工具造成的工作区改动不会回滚。
                  </span>
                </div>
              </section>
            )}

            {settingsTab === "files" && (
              <section className="settings-section settings-files">
                <div className="settings-summary-row">
                  <span className="field-label">agent_dir</span>
                  <span className="mono">~/.pipi/agents/{agent.name}/</span>
                </div>
                <div className="settings-summary-row">
                  <span className="field-label">mcp_servers</span>
                  <div className="settings-tags">
                    {agent.mcpServers.length
                      ? agent.mcpServers.map((server) => <span className="tag" key={server.name}>{server.name}</span>)
                      : <span className="dim">未配置</span>}
                  </div>
                </div>
                <AgentFileEditor
                  agentName={agent.name}
                  onError={onError}
                  disabled={!context.ready || context.running}
                  onSaved={async () => {
                    await onSaved();
                    await resetTestSession();
                  }}
                />
              </section>
            )}

            {settingsTab === "test" && <SessionDiagnostics context={context} />}
          </div>

          {(settingsTab === "basic" || settingsTab === "permissions") && (
            <div className="agent-settings-footer">
              <div className="save-state">
                {context.running
                  ? "测试运行中，请先停止"
                  : configurationDirty ? "有未保存的 Agent 配置" : "配置已保存"}
              </div>
              <button
                type="button"
                className="btn primary"
                disabled={saving || context.running || !context.ready || !configurationDirty || tools.length === 0}
                onClick={() => void saveConfiguration(context.running)}
              >
                {saving ? "保存中…" : "保存配置"}
              </button>
            </div>
          )}
        </div>
      )}
    />
  );
}

// ============ Agent 文件编辑器（AGENTS.md / memory/*.md） ============

interface AgentFileEditorProps {
  agentName: string;
  onError: (msg: string) => void;
  disabled?: boolean;
  onSaved?: () => void | Promise<void>;
}

function AgentFileEditor({ agentName, onError, disabled = false, onSaved }: AgentFileEditorProps) {
  const [files, setFiles] = useState<string[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [savedContent, setSavedContent] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [newFileName, setNewFileName] = useState("");

  const openFile = useCallback(async (relPath: string) => {
    setActiveFile(relPath);
    setLoading(true);
    try {
      const text = await invoke<string>("read_agent_file", {
        agentName,
        relPath,
      });
      setContent(text);
      setSavedContent(text);
    } catch (errorValue) {
      setActiveFile(null);
      onError(formatRuntimeError(errorValue));
    } finally {
      setLoading(false);
    }
  }, [agentName, onError]);

  const refreshFiles = useCallback(async () => {
    try {
      const list = await invoke<string[]>("list_agent_files", { agentName });
      setFiles(list);
      return list;
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
      return [];
    }
  }, [agentName, onError]);

  useEffect(() => {
    void (async () => {
      const list = await refreshFiles();
      // 默认打开 AGENTS.md（存在时）
      if (list.includes("AGENTS.md")) void openFile("AGENTS.md");
    })();
    // agentName 变化时重置
    setActiveFile(null);
    setContent("");
    setSavedContent("");
    setNewFileName("");
  }, [refreshFiles, openFile, agentName]);

  const saveFile = async () => {
    if (!activeFile || saving || disabled || content === savedContent) return;
    setSaving(true);
    try {
      await invoke("write_agent_file", {
        agentName,
        relPath: activeFile,
        content,
      });
      setSavedContent(content);
      await onSaved?.();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSaving(false);
    }
  };

  const createMemoryFile = async () => {
    if (disabled) return;
    const name = newFileName.trim().replace(/\.md$/, "");
    if (!name || name.includes("/")) return;
    const relPath = `memory/${name}.md`;
    if (files.includes(relPath)) {
      void openFile(relPath);
      setNewFileName("");
      return;
    }
    try {
      await invoke("write_agent_file", { agentName, relPath, content: "" });
      setNewFileName("");
      await refreshFiles();
      await openFile(relPath);
      await onSaved?.();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    }
  };

  const dirty = activeFile !== null && content !== savedContent;

  return (
    <div className="drow file-editor-row">
      <span className="k">AGENTS.md · memory/</span>
      <div className="v">
        <div className="file-tabs">
          {files.map((file) => (
            <button
              key={file}
              type="button"
              className={`file-tab mono${file === activeFile ? " active" : ""}`}
              disabled={disabled}
              onClick={() => void openFile(file)}
            >
              {file}
            </button>
          ))}
        </div>
        <div className="file-new">
          <input
            className="mono"
            value={newFileName}
            disabled={disabled}
            onChange={(e) => setNewFileName(e.target.value)}
            placeholder="新建 memory 文件，例如 user-prefs"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void createMemoryFile();
              }
            }}
          />
          <button
            type="button"
            className="btn ghost"
            disabled={disabled || !newFileName.trim() || newFileName.trim().includes("/")}
            onClick={() => void createMemoryFile()}
          >
            新建
          </button>
        </div>
        <textarea
          className="mono file-editor"
          value={loading ? "加载中…" : content}
          onChange={(e) => setContent(e.target.value)}
          disabled={disabled || !activeFile || loading}
          placeholder={activeFile ? undefined : "选择或新建一个文件开始编辑"}
          rows={8}
          spellCheck={false}
        />
        <div className="hint">
          {activeFile ? (
            <>
              {activeFile} —— AGENTS.md 每次对话开始时注入为系统指令；memory
              索引常驻上下文、正文由模型按需读取。改动需手动保存。
            </>
          ) : (
            "AGENTS.md 是系统级指令，memory/ 是 Agent 的持久记忆"
          )}
        </div>
        <button
          type="button"
          className="btn primary"
          disabled={disabled || !dirty || saving}
          onClick={() => void saveFile()}
        >
          {saving ? "保存中…" : disabled ? "测试运行中" : dirty ? "保存文件" : "已保存"}
        </button>
      </div>
    </div>
  );
}

// ============ 新建 Agent ============

interface CreateFormProps {
  providers: ProviderConfig[];
  defaultProviderId: string | null;
  onCreated: (name: string) => void | Promise<void>;
  onCancel: () => void;
  onError: (msg: string | null) => void;
}

function CreateForm({
  providers,
  defaultProviderId,
  onCreated,
  onCancel,
  onError,
}: CreateFormProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [providerId, setProviderId] = useState<string>(() => {
    if (defaultProviderId && providers.some((p) => p.id === defaultProviderId)) {
      return defaultProviderId;
    }
    return providers[0]?.id ?? "";
  });
  const [modelId, setModelId] = useState("");
  // 目录里选的模型：用它回填 provider 的 maxTokens / contextWindow
  const [pickedModel, setPickedModel] = useState<CatalogModel | undefined>(undefined);
  // Agent 组合工具具有跨 Agent 的持久副作用，必须显式勾选；新建 Agent
  // 继续只默认启用原来的基础工具。
  const [tools, setTools] = useState<string[]>([...DEFAULT_TOOLS]);
  const [bashMode, setBashMode] = useState<BashMode>("allowAll");
  const [commands, setCommands] = useState("");
  const [sandbox, setSandbox] = useState<SandboxMode>("workspace-write");
  const [submitting, setSubmitting] = useState(false);

  const toggleTool = (tool: string) => {
    setTools((prev) =>
      prev.includes(tool) ? prev.filter((t) => t !== tool) : [...prev, tool],
    );
  };

  const submit = async () => {
    if (!name.trim() || submitting) return;
    setSubmitting(true);
    onError(null);
    try {
      const permissions: PermissionsConfig = {
        tools,
        bash: {
          mode: bashMode,
          commands:
            bashMode === "allowAll"
              ? []
              : commands
                  .split("\n")
                  .map((c) => c.trim())
                  .filter(Boolean),
        },
        sandbox,
      };

      const selectedProvider = providers.find((p) => p.id === providerId);
      const trimmedModel = modelId.trim();
      const provider = selectedProvider && trimmedModel
        ? {
            id: trimmedModel,
            name: trimmedModel,
            api: selectedProvider.api,
            baseUrl: selectedProvider.baseUrl,
            maxTokens: resolveMaxTokens(pickedModel, 8192),
            contextWindow: resolveContextWindow(pickedModel, 0),
          }
        : null;

      await invoke("create_agent", {
        name,
        description,
        workspace: workspace.trim() || null,
        permissions,
        model: trimmedModel || null,
        provider,
      });
      await onCreated(name.trim());
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="screen">
      <div className="screen-bar">
        <button type="button" className="icon-btn" title="返回" onClick={onCancel}>
          <IconBack />
        </button>
        <span className="crumb">
          新建 Agent · <b>~/.pipi/agents/&lt;name&gt;/</b>
        </span>
      </div>
      <div className="detail">
        <div className="form">
          <div>
            <h2>新建 Agent</h2>
            <div className="sub">
              一切皆文件：将在 <span className="mono">~/.pipi/agents/&lt;name&gt;/</span>{" "}
              下生成 <span className="mono">agent.json</span>、
              <span className="mono">AGENTS.md</span>、
              <span className="mono">skills/</span>、<span className="mono">memory/</span>
              、<span className="mono">sessions/</span>
            </div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-name">名称</label>
            <input
              id="agent-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="例如：code-reviewer"
              autoFocus
            />
            <div className="hint">仅限字母、数字、- 和 _，将作为目录名</div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-desc">描述</label>
            <textarea
              id="agent-desc"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="这个 Agent 是做什么的？"
            />
          </div>

          <div className="field">
            <label className="label">默认模型</label>
            <div className="bind-row">
              <div className="provider-picker">
                <ChoiceSelect
                  id="agent-create-provider"
                  value={providerId}
                  choices={providers.map((p) => ({ value: p.id, label: p.name }))}
                  placeholder="（未绑定提供商）"
                  isClearable
                  menuInPortal
                  onChange={(next) => {
                    setProviderId(next);
                    // 同上：换供应商先清模型与目录限额，避免继承上一家的状态
                    setModelId("");
                    setPickedModel(undefined);
                  }}
                />
              </div>
              <ModelPicker
                id="agent-create-model"
                providers={providers}
                providerId={providerId}
                modelId={modelId}
                placeholder="模型 ID，如 claude-sonnet-4-5 / gpt-4o"
                onModelChange={(nextId, model) => {
                  setModelId(nextId);
                  setPickedModel(model);
                }}
              />
            </div>
            <div className="hint">新建会话时将默认使用此模型；也可留空稍后在详情页配置</div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-workspace">工作目录</label>
            <input
              id="agent-workspace"
              className="mono"
              value={workspace}
              onChange={(e) => setWorkspace(e.target.value)}
              placeholder="例如：~/projects/my-app"
            />
            <div className="hint">
              Agent 只在此目录内工作；留空使用默认的 agent 目录内 workspace/
            </div>
          </div>

          <div className="field">
            <span className="label">工具</span>
            <div className="tool-row">
              {KNOWN_TOOLS.map((tool) => (
                <label key={tool} className="tool-check">
                  <input
                    type="checkbox"
                    checked={tools.includes(tool)}
                    onChange={() => toggleTool(tool)}
                  />
                  <span className="mono">{tool}</span>
                </label>
              ))}
            </div>
          </div>

          <div className="field">
            <span className="label">沙箱</span>
            <div className="tool-row">
              {(Object.keys(SANDBOX_LABELS) as SandboxMode[]).map((mode) => (
                <label key={mode} className="tool-check">
                  <input
                    type="radio"
                    name="sandbox"
                    checked={sandbox === mode}
                    onChange={() => setSandbox(mode)}
                  />
                  <span>{SANDBOX_LABELS[mode]}</span>
                </label>
              ))}
            </div>
            <div className="hint">
              只读：不执行命令、不写文件；工作目录内可写：强制删除类命令与越出
              工作目录的写入被拒绝；完全访问：不设限
            </div>
          </div>

          <div className="field">
            <span className="label">命令权限（bash）</span>
            <div className="tool-row">
              {(Object.keys(BASH_MODE_LABELS) as BashMode[]).map((mode) => (
                <label key={mode} className="tool-check">
                  <input
                    type="radio"
                    name="bash-mode"
                    checked={bashMode === mode}
                    onChange={() => setBashMode(mode)}
                  />
                  <span>{BASH_MODE_LABELS[mode]}</span>
                </label>
              ))}
            </div>
            {bashMode !== "allowAll" && (
              <div className="perm-editor">
                <textarea
                  value={commands}
                  onChange={(e) => setCommands(e.target.value)}
                  placeholder={bashMode === "allowlist" ? "git\nnpm run\nls" : "rm\nsudo"}
                />
                <div className="hint">
                  {bashMode === "allowlist"
                    ? "每行一条；单词条目匹配以该词开头的命令，带空格按前缀匹配"
                    : "每行一条；命中任意条目的命令将被拒绝"}
                </div>
              </div>
            )}
          </div>

          <div className="actions">
            <button className="btn ghost" onClick={onCancel}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={!name.trim() || tools.length === 0 || submitting}
              onClick={submit}
            >
              {submitting ? "创建中…" : "创建"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ============ 设置（主区 tab，与对话界面同构：顶栏 + 左导航 + 内容） ============

/** 设置分组：导航用它划分不同类别。 */
type SettingsSection = "appearance" | "context" | "retry" | "providers" | "account";

const SETTINGS_SECTIONS: Record<SettingsSection, { label: string; description: string }> = {
  appearance: {
    label: "外观",
    description: "界面主题，只影响本机显示，不改动 Agent 配置。",
  },
  context: {
    label: "上下文",
    description: "长会话触碰窗口上限时的压缩方式：分叉出新会话，或原地替换旧轮次。",
  },
  retry: {
    label: "重试",
    description:
      "请求失败后的重发策略。只重发可重试的错误（限流、5xx、连接中断、流被截断）；"
      + "请求不合法、鉴权失败、上下文超限、配额耗尽一律不重发。已经输出正文的那一轮也不会重放。",
  },
  providers: {
    label: "模型提供商",
    description: "端点与密钥。Agent 绑定其中一个提供商，再选具体模型。",
  },
  account: {
    label: "账户",
    description: "Web 访问保护凭据。",
  },
};

interface SettingsViewProps {
  settings: Settings;
  onChange: (next: Settings) => void | Promise<void>;
  onClose: () => void;
  showLogout?: boolean;
}

function SettingsView({ settings, onChange, onClose, showLogout }: SettingsViewProps) {
  const [section, setSection] = useState<SettingsSection>("appearance");
  const [editing, setEditing] = useState<ProviderConfig | "new" | null>(null);
  const [preset, setPreset] = useState<CatalogProvider | null>(null);
  const [picking, setPicking] = useState(false);
  const [catalogNote, setCatalogNote] = useState<string | null>(null);
  const [refreshingCatalog, setRefreshingCatalog] = useState(false);
  const [draft, setDraft] = useState(settings);
  const draftRef = useRef(settings);

  /** 手动刷新模型目录（默认 24h 才自动刷新，这里给用户一个立即刷新的出口）。 */
  const refreshCatalog = async () => {
    if (refreshingCatalog) return;
    setRefreshingCatalog(true);
    setCatalogNote(null);
    try {
      resetCatalog();
      const next = await loadCatalog(true);
      setCatalogNote(`已刷新：${next.providers.length} 家提供商 · 来源 ${catalogSourceLabel(next)}`);
    } catch (reason) {
      setCatalogNote(`刷新失败：${reason instanceof Error ? reason.message : String(reason)}`);
    } finally {
      setRefreshingCatalog(false);
    }
  };

  const closeEditor = () => {
    setEditing(null);
    setPreset(null);
    setPicking(false);
  };

  useEffect(() => {
    draftRef.current = settings;
    setDraft(settings);
  }, [settings]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const commit = (update: (previous: Settings) => Settings) => {
    const next = update(draftRef.current);
    draftRef.current = next;
    setDraft(next);
    void onChange(next);
  };

  const saveProvider = (provider: ProviderConfig) => {
    commit((previous) => {
      const providers = [...previous.providers];
      const index = providers.findIndex((item) => item.id === provider.id);
      if (index >= 0) providers[index] = provider;
      else providers.push(provider);
      return { ...previous, providers };
    });
    closeEditor();
  };

  const deleteProvider = (id: string) => {
    if (!confirm(`删除提供商「${id}」？（已绑定它的 Agent 不受影响，但需重新配置）`)) return;
    commit((previous) => ({
      ...previous,
      providers: previous.providers.filter((provider) => provider.id !== id),
      defaultProviderId: previous.defaultProviderId === id ? null : previous.defaultProviderId,
    }));
  };

  const sections: SettingsSection[] = showLogout
    ? ["appearance", "context", "retry", "providers", "account"]
    : ["appearance", "context", "retry", "providers"];

  /** 导航项右侧的一行状态摘要，让分组一眼可辨。 */
  const sectionMeta = (id: SettingsSection): string => {
    switch (id) {
      case "appearance":
        return draft.theme === "dark" ? "深色" : "浅色";
      case "context":
        return draft.compaction.forkBeforeCompact ? "压缩前分叉" : "原地压缩";
      case "retry":
        return `${draft.retry.maxAttempts} 次尝试 · ${formatRetryDelay(draft.retry.baseDelayMs)}起`;
      case "providers": {
        const fallback = draft.providers.find((item) => item.id === draft.defaultProviderId);
        return `${draft.providers.length} 家 · 默认 ${fallback?.name ?? "未设置"}`;
      }
      case "account":
        return "Web 访问保护";
    }
  };

  return (
    <div className="screen settings-view">
      <div className="screen-bar">
        <button
          type="button"
          className="icon-btn"
          title="返回"
          aria-label="返回设置前的界面"
          onClick={onClose}
        >
          <IconBack />
        </button>
        <span className="crumb">
          设置 · <b>{SETTINGS_SECTIONS[section].label}</b>
        </span>
        <span className="spacer" />
        <span className="hint">改动即时保存</span>
      </div>

      <div className="settings-body">
        <nav
          className="settings-nav"
          role="tablist"
          aria-label="设置分组"
          aria-orientation="vertical"
        >
          {sections.map((id) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={section === id}
              className={`settings-nav-item${section === id ? " active" : ""}`}
              onClick={() => setSection(id)}
            >
              <span className="name">{SETTINGS_SECTIONS[id].label}</span>
              <span className="meta">{sectionMeta(id)}</span>
            </button>
          ))}
        </nav>

        <div className="settings-main">
          <div className="settings-main-head">
            <h2>{SETTINGS_SECTIONS[section].label}</h2>
            <p>{SETTINGS_SECTIONS[section].description}</p>
          </div>

          <div className="settings-main-body">
            {section === "appearance" && (
              <section className="settings-block">
                <span className="label">主题</span>
                <div className="theme-row">
                  <ThemeOption
                    active={draft.theme === "dark"}
                    name="深色"
                    swatch={["#171717", "#212121", "#0169CC", "#ececec"]}
                    onClick={() => commit((previous) => ({ ...previous, theme: "dark" }))}
                  />
                  <ThemeOption
                    active={draft.theme === "light"}
                    name="浅色"
                    swatch={["#f9f9f9", "#ffffff", "#0169CC", "#0d0d0d"]}
                    onClick={() => commit((previous) => ({ ...previous, theme: "light" }))}
                  />
                </div>
              </section>
            )}

            {section === "context" && (
              <section className="settings-block">
                <span className="label">上下文压缩</span>
                <div className="setting-checks">
                  <label className="tool-check">
                    <input
                      type="checkbox"
                      checked={draft.compaction.forkBeforeCompact}
                      onChange={() =>
                        commit((previous) => ({
                          ...previous,
                          compaction: {
                            ...previous.compaction,
                            forkBeforeCompact: !previous.compaction.forkBeforeCompact,
                          },
                        }))
                      }
                    />
                    <span>压缩前分叉新会话（原会话保留为完整记录）</span>
                  </label>
                  <label className="tool-check">
                    <input
                      type="checkbox"
                      checked={draft.compaction.archiveOriginal}
                      disabled={!draft.compaction.forkBeforeCompact}
                      onChange={() =>
                        commit((previous) => ({
                          ...previous,
                          compaction: {
                            ...previous.compaction,
                            archiveOriginal: !previous.compaction.archiveOriginal,
                          },
                        }))
                      }
                    />
                    <span>分叉后归档原会话</span>
                  </label>
                  <div className="hint">关闭分叉即回到原地压缩：摘要会替换当前会话里被压缩的旧轮次。</div>
                </div>
              </section>
            )}

            {section === "retry" && (
              <section className="settings-block">
                <span className="label">请求重试</span>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="retry-max-attempts">最大尝试次数</label>
                  <div className="inline-value">
                    <input
                      id="retry-max-attempts"
                      className="mono"
                      type="number"
                      min={1}
                      max={5}
                      value={draft.retry.maxAttempts}
                      onChange={(event) =>
                        commit((previous) => ({
                          ...previous,
                          retry: {
                            ...previous.retry,
                            maxAttempts: Number(event.target.value) || 1,
                          },
                        }))
                      }
                    />
                    <span className="unit">次（含首次）</span>
                  </div>
                  <span className="hint">1 = 不重试；上限 5。</span>
                </div>

                <div className="settings-field compact-threshold-field">
                  <label htmlFor="retry-base-delay">起始退避</label>
                  <div className="inline-value">
                    <input
                      id="retry-base-delay"
                      className="mono"
                      type="number"
                      min={100}
                      step={100}
                      value={draft.retry.baseDelayMs}
                      onChange={(event) =>
                        commit((previous) => ({
                          ...previous,
                          retry: {
                            ...previous.retry,
                            baseDelayMs: Number(event.target.value) || 100,
                          },
                        }))
                      }
                    />
                    <span className="unit">毫秒</span>
                  </div>
                  <span className="hint">第 n 次失败后等待 起始退避 × 2ⁿ⁻¹（带抖动）。</span>
                </div>

                <div className="settings-field compact-threshold-field">
                  <label htmlFor="retry-max-delay">退避上限</label>
                  <div className="inline-value">
                    <input
                      id="retry-max-delay"
                      className="mono"
                      type="number"
                      min={1000}
                      step={1000}
                      value={draft.retry.maxDelayMs}
                      onChange={(event) =>
                        commit((previous) => ({
                          ...previous,
                          retry: {
                            ...previous.retry,
                            maxDelayMs: Number(event.target.value) || 1000,
                          },
                        }))
                      }
                    />
                    <span className="unit">毫秒</span>
                  </div>
                  <span className="hint">
                    服务端要求等待更久时直接放弃（不会静默等待）；无进展超时最多额外重试 1 次。
                  </span>
                </div>
              </section>
            )}

            {section === "providers" && (
              <section className="settings-block">
                <span className="label">模型提供商</span>
                {draft.providers.map((provider) => {
                  const status = keyStatus(provider);
                  return (
                    <div className="provider-row" key={provider.id}>
                      <div className="info">
                        <div className="p-name">
                          {provider.name}
                          <span className="badge neutral">{API_LABELS[provider.api]}</span>
                          <span className={`badge ${status.warn ? "warn" : "neutral"}`}>
                            {status.label}
                          </span>
                          {draft.defaultProviderId === provider.id && (
                            <span className="badge">默认</span>
                          )}
                        </div>
                        <div className="p-url mono">{provider.baseUrl}</div>
                      </div>
                      <div className="p-actions">
                        <button
                          type="button"
                          className="link"
                          onClick={() => {
                            setPreset(null);
                            setPicking(false);
                            setEditing(provider);
                          }}
                        >
                          编辑
                        </button>
                        {draft.defaultProviderId !== provider.id && (
                          <button
                            type="button"
                            className="link"
                            onClick={() => commit((previous) => ({ ...previous, defaultProviderId: provider.id }))}
                          >
                            设为默认
                          </button>
                        )}
                        <button type="button" className="link danger" onClick={() => deleteProvider(provider.id)}>
                          删除
                        </button>
                      </div>
                    </div>
                  );
                })}

                {editing === null && !picking && (
                  <div className="preset-actions">
                    <button type="button" className="btn ghost" onClick={() => setPicking(true)}>
                      ＋ 从预设添加
                    </button>
                    <button
                      type="button"
                      className="link"
                      onClick={() => {
                        setPreset(null);
                        setEditing("new");
                      }}
                    >
                      手动配置端点
                    </button>
                    <span className="spacer" />
                    <button
                      type="button"
                      className="link"
                      disabled={refreshingCatalog}
                      onClick={() => void refreshCatalog()}
                    >
                      {refreshingCatalog ? "刷新中…" : "刷新模型目录"}
                    </button>
                    {catalogNote && <span className="hint">{catalogNote}</span>}
                  </div>
                )}

                {editing === null && picking && (
                  <PresetPicker
                    existingIds={draft.providers.map((provider) => provider.id)}
                    onPick={(next) => {
                      setPreset(next);
                      setPicking(false);
                      setEditing("new");
                    }}
                    onCancel={() => setPicking(false)}
                  />
                )}

                {editing !== null && (
                  <ProviderForm
                    initial={editing === "new" ? null : editing}
                    preset={editing === "new" ? preset : null}
                    existingIds={draft.providers.map((provider) => provider.id)}
                    onSave={saveProvider}
                    onCancel={closeEditor}
                  />
                )}
              </section>
            )}

            {section === "account" && showLogout && (
              <section className="settings-block">
                <span className="label">远程访问凭据</span>
                <div className="provider-row">
                  <div className="info">
                    <div className="p-name">
                      Web 访问保护
                      <span className="badge neutral">已认证</span>
                    </div>
                    <div className="p-url mono">PIPI_AUTH_TOKEN 已验证</div>
                  </div>
                  <div className="p-actions">
                    <button
                      type="button"
                      className="link danger"
                      onClick={async () => {
                        await logout();
                        onClose();
                      }}
                    >
                      退出登录
                    </button>
                  </div>
                </div>
              </section>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function ConfirmModal({
  title,
  message,
  confirmLabel = "彻底删除",
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmLabel?: string;
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onCancel]);

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal confirm-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="confirm-title">{title}</h2>
          <button type="button" className="icon-btn close-btn" onClick={onCancel} title="取消">
            <IconClose />
          </button>
        </div>
        <div className="modal-section">
          <p className="confirm-message">{message}</p>
        </div>
        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onCancel} disabled={busy}>
            取消
          </button>
          <button
            type="button"
            className="btn danger"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm();
              } finally {
                setBusy(false);
                onCancel();
              }
            }}
          >
            {busy ? "删除中…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

function ThemeOption({
  active,
  name,
  swatch,
  onClick,
}: {
  active: boolean;
  name: string;
  swatch: string[];
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`theme-option${active ? " active" : ""}`}
      aria-pressed={active}
      onClick={onClick}
    >
      <div className="swatch">
        {swatch.map((color) => (
          <span key={color} style={{ background: color }} />
        ))}
      </div>
      <div className="name">
        {name}
        {active && <span style={{ color: "var(--accent-text)" }}> ✓</span>}
      </div>
    </button>
  );
}

interface PresetPickerProps {
  existingIds: string[];
  onPick: (provider: CatalogProvider) => void;
  onCancel: () => void;
}

/**
 * 预设选择器：选项来自模型目录（models.dev），按分组列出。
 * 搜索与键盘导航交给 react-select；这里只标注「已添加」和来源。
 */
function PresetPicker({ existingIds, onPick, onCancel }: PresetPickerProps) {
  const [catalog, setCatalog] = useState<ModelCatalog | null>(peekCatalog());
  const [loading, setLoading] = useState(peekCatalog() === null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (peekCatalog()) return;
    let alive = true;
    setLoading(true);
    loadCatalog(attempt > 0)
      .then((next) => {
        if (!alive) return;
        setCatalog(next);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (alive) setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [attempt]);

  const groups = catalog
    ? providerGroups(catalog).map((entry) => ({
        label: entry.label,
        options: entry.options.map((option) => {
          const provider = catalog.providers.find((item) => item.id === option.value);
          const parts = [option.label];
          if (provider?.api === "anthropic-messages") parts.push("Anthropic 协议");
          if (existingIds.includes(option.value)) parts.push("已添加");
          return { value: option.value, label: parts.join(" · ") };
        }),
      }))
    : [];

  return (
    <div className="preset-picker">
      <div className="preset-picker-head">
        <span className="label">选择提供商预设</span>
        <span className="spacer" />
        <button type="button" className="link" onClick={onCancel}>
          取消
        </button>
      </div>

      <div className="preset-picker-body">
        <ChoiceSelect
          value=""
          groups={groups}
          disabled={loading || !catalog}
          placeholder={loading ? "正在加载模型目录…" : "搜索提供商（名称 / id）…"}
          ariaLabel="选择提供商预设"
          autoFocus
          menuInPortal
          onChange={(id) => {
            const hit = catalog?.providers.find((provider) => provider.id === id);
            if (hit) onPick(hit);
          }}
        />
        {error && (
          <div className="form-warning">
            {error}
            <button type="button" className="link" onClick={() => setAttempt((n) => n + 1)}>
              重试
            </button>
          </div>
        )}
        {catalog && (
          <div className="hint">
            {catalog.providers.length} 家提供商 · 来源 {catalogSourceLabel(catalog)}；
            预设只填端点、协议与环境变量名，密钥始终由你提供（环境变量优先，其次明文）。
          </div>
        )}
      </div>
    </div>
  );
}

interface ProviderFormProps {
  initial: ProviderConfig | null;
  /** 从目录预设添加时的种子值（不含密钥）；编辑已有提供商时为 null。 */
  preset: CatalogProvider | null;
  existingIds: string[];
  onSave: (p: ProviderConfig) => void;
  onCancel: () => void;
}

function ProviderForm({ initial, preset, existingIds, onSave, onCancel }: ProviderFormProps) {
  const seed = preset ? providerSeed(preset) : null;
  const [name, setName] = useState(initial?.name ?? seed?.name ?? "");
  const [api, setApi] = useState<ApiKind>(initial?.api ?? seed?.api ?? "anthropic-messages");
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? seed?.baseUrl ?? "");
  const [envKey, setEnvKey] = useState(initial?.envKey ?? seed?.envKey ?? "");
  const [apiKey, setApiKey] = useState(initial?.apiKey ?? "");

  // 预设默认沿用预设 id；但用户一改名称就改由名称派生（与手动路径一致），
  // 否则「改个名字换个 id」这条出路在预设路径上不成立。
  const nameEdited = Boolean(preset) && name.trim() !== preset!.name;
  const id = initial?.id ?? (seed && !nameEdited ? seed.id : slugify(name));

  const valid =
    name.trim().length > 0 &&
    baseUrl.trim().startsWith("http") &&
    (initial !== null || !existingIds.includes(id));

  const idTaken = initial === null && existingIds.includes(id);

  const submit = () => {
    if (!valid) return;
    onSave({
      id,
      name: name.trim(),
      api,
      baseUrl: baseUrl.trim().replace(/\/+$/, ""),
      envKey: envKey.trim() || null,
      apiKey: apiKey.trim() || null,
    });
  };

  return (
    <div className="provider-form">
      {preset && (
        <div className="preset-banner">
          <div className="preset-banner-head">
            <span className="badge">目录预设</span>
            <span className="preset-banner-name">{preset.name}</span>
            <span className="badge neutral">{preset.group}</span>
            <span className="hint">
              {preset.models.length > 0
                ? `${preset.models.length} 个可工具调用的模型`
                : "本地运行时：模型名手填"}
            </span>
            {preset.doc && (
              <a className="link" href={preset.doc} target="_blank" rel="noreferrer">
                文档 ↗
              </a>
            )}
          </div>
          {preset.note && <div className="hint">{preset.note}</div>}
          {preset.baseUrlNote && (
            <div className="hint mono">端点说明：{preset.baseUrlNote}</div>
          )}
        </div>
      )}
      <div className="grid">
        <div className="form-row">
          <label className="label">名称</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="例如：DeepSeek"
            autoFocus
          />
          <div className="hint mono">{initial ? `id: ${id}` : `id: ${id || "…"}`}</div>
        </div>
        <div className="form-row">
          <label className="label">API 协议</label>
          <select value={api} onChange={(e) => setApi(e.target.value as ApiKind)}>
            {(Object.keys(API_LABELS) as ApiKind[]).map((k) => (
              <option key={k} value={k}>
                {API_LABELS[k]}
              </option>
            ))}
          </select>
        </div>
        <div className="form-row full">
          <label className="label">Base URL</label>
          <input
            className="mono"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://api.deepseek.com/v1"
          />
        </div>
        <div className="form-row">
          <label className="label">密钥环境变量（推荐）</label>
          <input
            className="mono"
            value={envKey}
            onChange={(e) => setEnvKey(e.target.value)}
            placeholder="DEEPSEEK_API_KEY"
          />
        </div>
        <div className="form-row">
          <label className="label">API Key（明文，本机自担）</label>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="留空则只用环境变量"
          />
        </div>
      </div>
      {idTaken && (
        <div className="form-warning">
          id「{id}」已存在：改一下名称即可换 id；或关闭本表单后直接「编辑」已有提供商。
        </div>
      )}
      <div className="actions">
        <button className="btn ghost" onClick={onCancel}>
          取消
        </button>
        <button className="btn primary" disabled={!valid} onClick={submit}>
          保存
        </button>
      </div>
    </div>
  );
}

function slugify(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\u4e00-\u9fff]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "provider-new";
}
