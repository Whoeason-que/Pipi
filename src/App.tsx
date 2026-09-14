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
import ChatView from "./Chat";
import Login from "./Login";
import { ScreenTabs } from "./ScreenTabs";
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

const KNOWN_TOOLS = ["read", "write", "edit", "bash", "memory", "glob", "grep"] as const;

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
  const [chatRunning, setChatRunning] = useState(false);
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
    setError(msg);
  }, []);

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
      const infoRequestId = ++sessionInfoRequestRef.current;
      void invoke<SessionInfoView | null>("session_info")
        .then((info) => {
          if (active && infoRequestId === sessionInfoRequestRef.current) setActiveSession(info);
        })
        .catch((errorValue) => {
          if (active) setError(formatRuntimeError(errorValue));
        });
      void refreshSessions(agentsRef.current.map((agent) => agent.name));
    });
    return () => {
      active = false;
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [refreshSessions]);

  const openSession = async (agentName: string, sessionId: string) => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再切换会话");
      return;
    }
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
          await invoke("new_session").catch(() => {});
          return;
        }
        setSelected(agentName);
        setCreating(false);
        setChatOpen(true);
        setSidebarOpen(false);
        setActiveSession(null);
        setChatKey((key) => key + 1);
        infoRequestId = ++sessionInfoRequestRef.current;
        const info = await invoke<SessionInfoView | null>("session_info");
        if (
          requestId === navigationRequestRef.current
          && infoRequestId === sessionInfoRequestRef.current
        ) {
          setActiveSession(info);
        } else {
          await invoke("new_session").catch(() => {});
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
        } else {
          await invoke("new_session").catch(() => {});
        }
      }
    });
  };

  const startNewSession = async (agentName: string) => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再新建会话");
      return;
    }
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    const previousSessionId = activeSessionRef.current?.sessionId;
    if (previousSessionId) blockedSessionIdsRef.current.add(previousSessionId);
    await enqueueNavigation(requestId, async () => {
      try {
        await invoke("new_session");
        if (requestId !== navigationRequestRef.current) return;
        setSelected(agentName);
        setCreating(false);
        setChatOpen(true);
        setSidebarOpen(false);
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

  const createSessionFromChat = async (previousSessionId?: string): Promise<boolean> => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再新建会话");
      return false;
    }
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    if (previousSessionId) blockedSessionIdsRef.current.add(previousSessionId);
    let accepted = false;
    await enqueueNavigation(requestId, async () => {
      try {
        await invoke("new_session");
        accepted = requestId === navigationRequestRef.current;
      } catch (errorValue) {
        if (requestId === navigationRequestRef.current) {
          if (previousSessionId) blockedSessionIdsRef.current.delete(previousSessionId);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
    return accepted && requestId === navigationRequestRef.current;
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
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再切换");
      return;
    }
    invalidateNavigation();
    // 释放后端会话槽（new_session 仅清空槽、不落盘），避免该 Agent 被占用检查锁住
    void invoke("new_session").catch(() => {});
    setSelected(agentName);
    setCreating(false);
    setChatOpen(false);
    setSidebarOpen(false);
  };

  const startCreating = () => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再新建 Agent");
      return;
    }
    invalidateNavigation();
    setCreating(true);
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
    <div className={`app${sidebarOpen ? " sidebar-open" : ""}`}>
      <div className="app-body">
        <aside className="sidebar" id="primary-navigation" aria-label="主导航">
          <div className="sb-head">
            <span className="wordmark">Pipi</span>
            {APP_VERSION && <span className="ver-chip">v{APP_VERSION}</span>}
            <span className="spacer" />
            <button
              type="button"
              className="icon-btn"
              title="设置"
              aria-label="打开设置"
              onClick={() => {
                setSidebarOpen(false);
                setSettingsOpen(true);
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
              const activeHere = activeSession?.agentName === a.name;
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
                    <span className="agent-count">{sessionsByAgent[a.name]?.length ?? 0}</span>
                    <span className="agent-actions">
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
                      const isCurrent = activeHere && activeSession?.sessionId === sess.id;
                      return (
                        <div
                          key={sess.id}
                          className={`session${isCurrent ? " active" : ""}`}
                          role="button"
                          tabIndex={0}
                          aria-current={isCurrent ? "true" : undefined}
                          title={`${sess.title}${sess.model ? ` · ${sess.model}` : ""}（${sess.messageCount} 条消息）`}
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
                          <span className="session-title">{sess.title}</span>
                          {sess.model && <span className="session-model">{sess.model}</span>}
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
                      <span className="session-model">{archivedSessions[a.name]?.length ?? ""}</span>
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
              {current?.name ?? (creating ? "新建 Agent" : "Pipi")}
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

          {creating ? (
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
                  blockedSessionIds={[...blockedSessionIdsRef.current]}
                  onBack={() => {
                    if (chatRunning) {
                      safeSetError("Agent 正在运行，请先停止后再返回");
                      return;
                    }
                    invalidateNavigation();
                    // 释放后端会话槽：否则「返回」后该 Agent 仍被占用，归档/删除会被拒
                    void invoke("new_session").catch(() => {});
                    setChatOpen(false);
                    setSidebarOpen(false);
                  }}
                  onShowDetail={() => {
                    if (chatRunning) {
                      safeSetError("Agent 正在运行，请先停止后再切换");
                      return;
                    }
                    invalidateNavigation();
                    void invoke("new_session").catch(() => {});
                    setChatOpen(false);
                  }}
                  onError={safeSetError}
                  onNewSession={createSessionFromChat}
                  onRunningChange={setChatRunning}
                  onSessionReset={(info) => {
                    setActiveSession(info ?? null);
                    void refreshSessions([current.name]);
                  }}
                />
              ) : (
                <AgentDetail
                  key={current.name}
                  agent={current}
                  providers={settings.providers}
                  onSaved={refresh}
                  onChat={() => void startNewSession(current.name)}
                  onError={safeSetError}
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
          运行中 <b>{chatRunning ? 1 : 0}</b>
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

      {settingsOpen && settings && (
        <SettingsModal
          settings={settings}
          onChange={updateSettings}
          onClose={() => setSettingsOpen(false)}
          showLogout={!isTauriRuntime() && authStatus.authRequired}
        />
      )}

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
  onChat: () => void;
  onError: (msg: string) => void;
}

function AgentDetail({ agent, providers, onSaved, onChat, onError }: AgentDetailProps) {
  const { bash } = agent.permissions;
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
  const [savingMeta, setSavingMeta] = useState(false);
  // agent 切换时重置编辑状态（否则上一个 Agent 的草稿会串台）
  const agentNameRef = useRef(agent.name);
  if (agentNameRef.current !== agent.name) {
    agentNameRef.current = agent.name;
    setDescription(agent.description);
    setWorkspace(agent.workspace ?? "");
    setTools(agent.permissions.tools);
    setBashMode(bash.mode);
    setCommands(bash.commands.join("\n"));
    setSandbox(agent.permissions.sandbox);
  }

  const bindProvider = async () => {
    if (saving) return;
    setSaving(true);
    try {
      const p = providers.find((x) => x.id === providerId);
      const next: AgentDefinition = {
        ...agent,
        model: modelId.trim(),
        provider: p
          ? {
              id: modelId.trim(),
              name: modelId.trim(),
              api: p.api,
              baseUrl: p.baseUrl,
              maxTokens: resolveMaxTokens(pickedModel, savedLimits.maxTokens),
              contextWindow: resolveContextWindow(pickedModel, savedLimits.contextWindow),
            }
          : null,
      };
      await invoke("save_agent", { def: next });
      setSavedLimits({
        maxTokens: next.provider?.maxTokens ?? 8192,
        contextWindow: next.provider?.contextWindow ?? 0,
      });
      await onSaved();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSaving(false);
    }
  };

  const saveMeta = async () => {
    if (savingMeta || tools.length === 0) return;
    setSavingMeta(true);
    try {
      const next: AgentDefinition = {
        ...agent,
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
      };
      await invoke("save_agent", { def: next });
      await onSaved();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSavingMeta(false);
    }
  };

  const metaDirty =
    description !== agent.description ||
    (workspace.trim() || null) !== (agent.workspace ?? null) ||
    tools.join(",") !== agent.permissions.tools.join(",") ||
    bashMode !== bash.mode ||
    (bashMode === "allowAll" ? "" : commands) !== bash.commands.join("\n") ||
    sandbox !== agent.permissions.sandbox;

  return (
    <div className="screen">
      <div className="screen-bar">
        <ScreenTabs
          active="detail"
          onSelect={(view) => {
            if (view === "chat") onChat();
          }}
        />
        <span className="spacer" />
        <button className="btn primary" onClick={onChat}>
          ▶ 开始对话
        </button>
      </div>

      <div className="detail">
        <div className="detail-head">
          <div>
            <h2>
              <span className="mono">{agent.name}</span>
              {agent.provider && <span className="badge">{agent.provider.id}</span>}
            </h2>
            <div className="sub">{agent.description || "（暂无描述）"}</div>
          </div>
        </div>

        <section className="dsec">
          <h4>模型</h4>
          <div className="drow">
            <span className="k">default_model</span>
            <div className="v">
              <div className="bind-row">
                <div className="provider-picker">
                  <ChoiceSelect
                    id="agent-provider-select"
                    value={providerId}
                    choices={providers.map((p) => ({ value: p.id, label: p.name }))}
                    placeholder="（未绑定提供商）"
                    isClearable
                    menuInPortal
                    onChange={(next) => {
                      setProviderId(next);
                      // 与会话模型弹窗同一护栏：换供应商必须清掉上一家的模型与限额，
                      // 否则会保存出「B 的端点 + A 的模型/上限」这种静默错配。
                      setModelId("");
                      setPickedModel(undefined);
                      setSavedLimits({ maxTokens: 8192, contextWindow: 0 });
                    }}
                  />
                </div>
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
                <button
                  className="btn primary"
                  disabled={saving || (!!providerId && !modelId.trim())}
                  onClick={bindProvider}
                >
                  {saving ? "保存中…" : "保存"}
                </button>
              </div>
              <span className="hint">新建会话将默认继承此模型；会话内可随时切换</span>
              {agent.provider && (
                <span className="hint mono">
                  {agent.provider.api} · {agent.provider.baseUrl}
                </span>
              )}
            </div>
          </div>
        </section>

        <section className="dsec">
          <h4>信息与权限</h4>
          <div className="drow">
            <span className="k">description</span>
            <div className="v">
              <textarea
                className="meta-editor"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="这个 Agent 是做什么的？"
                rows={2}
              />
            </div>
          </div>
          <div className="drow">
            <span className="k">workspace</span>
            <div className="v">
              <input
                className="mono"
                value={workspace}
                onChange={(e) => setWorkspace(e.target.value)}
                placeholder={`~/.pipi/agents/${agent.name}/workspace（默认）`}
              />
            </div>
          </div>
          <div className="drow">
            <span className="k">tools</span>
            <div className="v">
              <div className="tool-row">
                {KNOWN_TOOLS.map((tool) => (
                  <label key={tool} className="tool-check">
                    <input
                      type="checkbox"
                      checked={tools.includes(tool)}
                      onChange={() =>
                        setTools((prev) =>
                          prev.includes(tool) ? prev.filter((t) => t !== tool) : [...prev, tool],
                        )
                      }
                    />
                    <span className="mono">{tool}</span>
                  </label>
                ))}
              </div>
            </div>
          </div>
          <div className="drow">
            <span className="k">bash.mode</span>
            <div className="v">
              <div className="tool-row">
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
          </div>
          <div className="drow">
            <span className="k">sandbox</span>
            <div className="v">
              <div className="tool-row">
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
                只读：不执行命令、不写文件；工作目录内可写：强制删除类命令与越出
                工作目录的写入被拒绝；完全访问：不设限。保存后对下一次会话生效。
              </span>
            </div>
          </div>
          <div className="drow">
            <span className="k" />
            <div className="v">
              <button
                className="btn primary"
                disabled={!metaDirty || tools.length === 0 || savingMeta}
                onClick={saveMeta}
              >
                {savingMeta ? "保存中…" : "保存信息与权限"}
              </button>
              {!metaDirty && <span className="sub">（无改动）</span>}
            </div>
          </div>
        </section>

        <section className="dsec">
          <h4>文件</h4>
          <div className="drow">
            <span className="k">agent_dir</span>
            <div className="v mono">
              ~/.pipi/agents/{agent.name}/
              <span className="sub">skills/ · sessions/ 由文件直接管理，改动即生效</span>
            </div>
          </div>
          <div className="drow">
            <span className="k">mcp_servers</span>
            <div className="v">
              {agent.mcpServers.length
                ? agent.mcpServers.map((m) => (
                    <span className="tag" key={m.name}>
                      {m.name}
                    </span>
                  ))
                : <span className="dim">未配置</span>}
            </div>
          </div>
          <AgentFileEditor agentName={agent.name} onError={onError} />
        </section>
      </div>
    </div>
  );
}

// ============ Agent 文件编辑器（AGENTS.md / memory/*.md） ============

interface AgentFileEditorProps {
  agentName: string;
  onError: (msg: string) => void;
}

function AgentFileEditor({ agentName, onError }: AgentFileEditorProps) {
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
    if (!activeFile || saving || content === savedContent) return;
    setSaving(true);
    try {
      await invoke("write_agent_file", {
        agentName,
        relPath: activeFile,
        content,
      });
      setSavedContent(content);
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSaving(false);
    }
  };

  const createMemoryFile = async () => {
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
            disabled={!newFileName.trim() || newFileName.trim().includes("/")}
            onClick={() => void createMemoryFile()}
          >
            新建
          </button>
        </div>
        <textarea
          className="mono file-editor"
          value={loading ? "加载中…" : content}
          onChange={(e) => setContent(e.target.value)}
          disabled={!activeFile || loading}
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
          disabled={!dirty || saving}
          onClick={() => void saveFile()}
        >
          {saving ? "保存中…" : dirty ? "保存文件" : "已保存"}
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
  const [tools, setTools] = useState<string[]>([...KNOWN_TOOLS]);
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

// ============ 设置弹窗 ============

interface SettingsModalProps {
  settings: Settings;
  onChange: (next: Settings) => void | Promise<void>;
  onClose: () => void;
  showLogout?: boolean;
}

function SettingsModal({ settings, onChange, onClose, showLogout }: SettingsModalProps) {
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

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="settings-title">设置</h2>
          <button type="button" className="icon-btn close-btn" onClick={onClose} title="关闭">
            <IconClose />
          </button>
        </div>

        <div className="modal-section">
          <span className="label">主题</span>
          <div className="theme-row">
            <ThemeOption
              active={draft.theme === "dark"}
              name="深色"
              swatch={["#101214", "#0b0c0d", "#0169CC", "#e6eaee"]}
              onClick={() => commit((previous) => ({ ...previous, theme: "dark" }))}
            />
            <ThemeOption
              active={draft.theme === "light"}
              name="浅色"
              swatch={["#f6f7f8", "#fbfbfc", "#0169CC", "#15181b"]}
              onClick={() => commit((previous) => ({ ...previous, theme: "light" }))}
            />
          </div>
        </div>

        <div className="modal-section">
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
        </div>

        {showLogout && (
          <div className="modal-section">
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
          </div>
        )}
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
