import { useCallback, useEffect, useRef, useState } from "react";
import AgentWorkbench from "./agents/AgentWorkbench";
import CreateAgentForm from "./agents/CreateAgentForm";
import { Rail, type NavSection } from "./app/Rail";
import { Sidebar } from "./app/Sidebar";
import { StatusBar } from "./app/StatusBar";
import { useArchiveActions, type ConfirmRequest } from "./app/use-archive-actions";
import { useHostEvents } from "./app/use-host-events";
import { useSessionList } from "./app/use-session-list";
import { useSessionNavigation } from "./app/use-session-navigation";
import { useSettings } from "./app/use-settings";
import ChatView from "./Chat";
import { formatRuntimeError, isAuthSilentError } from "./chat-runtime";
import ConfirmModal from "./components/ConfirmModal";
import EmptyState from "./components/EmptyState";
import { IconClose, IconMenu } from "./icons";
import Login from "./Login";
import {
  getAuthStatus,
  getConnectionState,
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
import { SkillsSidebar } from "./skills/SkillsSidebar";
import { SkillsView } from "./skills/SkillsView";
import { applyTheme } from "./theme";
import {
  type SessionInfoView,
  type SessionSummaryView,
  type SkillLibrary
} from "./types";

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
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [chatOpen, setChatOpen] = useState(false);
  const [chatKey, setChatKey] = useState(0);
  const [activeSession, setActiveSession] = useState<SessionInfoView | null>(null);
  /** 当前查看的会话 id（null = 新会话，还没落文件）。 */
  const [viewSessionId, setViewSessionId] = useState<string | null>(null);
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
  /** 技能库：store 一览 + 当前查看的技能（「技能」分区用）。 */
  const [skillLibrary, setSkillLibrary] = useState<SkillLibrary | null>(null);
  const [skillSelected, setSkillSelected] = useState<string | null>(null);
  const [skillLoadError, setSkillLoadError] = useState<string | null>(null);
  const [connection, setConnection] = useState<ConnectionState>(getConnectionState());
  const [error, setError] = useState<string | null>(null);
  // 删除确认弹窗（window.confirm 在 Tauri WebView 不可用，一律走自定义弹层）
  const [confirmState, setConfirmState] = useState<ConfirmRequest | null>(null);
  const selectedRef = useRef<string | null>(null);
  const activeSessionRef = useRef<SessionInfoView | null>(null);

  selectedRef.current = selected;
  activeSessionRef.current = activeSession;

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

  const {
    agents,
    archivedAgents,
    sessionsByAgent,
    refresh,
    refreshSessions,
    refreshArchived,
    removeAgentSessions,
    sessionsFor,
    sessionTotal,
  } = useSessionList(safeSetError);

  const { settings, setSettings, updateSettings } = useSettings(setError);

  const navigation = useSessionNavigation({
    setError,
    selectedRef,
    activeSessionRef,
    setSelected,
    setCreating,
    setSettingsOpen,
    setChatOpen,
    setSidebarOpen,
    setExpandedAgents,
    setViewSessionId,
    setActiveSession,
    setChatKey,
  });

  const {
    runningSessions,
    handleRunningChange,
    isConversationRunning,
    agentHasRunning,
    forgetRunningSession,
    forgetRunningAgent,
  } = useHostEvents({
    sessionRuntime,
    refreshSessions,
    safeSetError,
    setError,
    setActiveSession,
    selectedRef,
    activeSessionRef,
    blockedSessionIdsRef: navigation.blockedSessionIdsRef,
    sessionInfoRequestRef: navigation.sessionInfoRequestRef,
  });

  useEffect(() => subscribeConnection(setConnection), []);

  /** 读取全局技能 store 一览（进入「技能」分区时，以及每次显式操作之后）。 */
  const loadSkillLibrary = useCallback(async (): Promise<SkillLibrary | null> => {
    try {
      const library = await invoke("skill_library");
      setSkillLibrary(library);
      setSkillLoadError(null);
      return library;
    } catch (errorValue) {
      setSkillLoadError(formatRuntimeError(errorValue));
      return null;
    }
  }, []);

  useEffect(() => {
    if (navSection !== "skills" || settingsOpen) return;
    void loadSkillLibrary();
  }, [navSection, settingsOpen, loadSkillLibrary]);

  // 技能被移除后清掉选中态（详情会退回过览）
  useEffect(() => {
    if (!skillSelected || !skillLibrary) return;
    if (!skillLibrary.skills.some((skill) => skill.name === skillSelected)) {
      setSkillSelected(null);
    }
  }, [skillLibrary, skillSelected]);

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
        if (isAuthSilentError(errorValue)) return;
        safeSetError(formatRuntimeError(errorValue));
      });
    return () => {
      active = false;
    };
  }, [authStatus.checking, authStatus.authRequired, authStatus.authenticated, refresh, refreshArchived, safeSetError, setSettings]);

  useEffect(() => {
    if (!sidebarOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSidebarOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [sidebarOpen]);

  // ============ 归档 / 恢复 / 删除 ============
  const {
    archiveAgent,
    restoreAgent,
    deleteAgent,
    archiveSession,
    deleteSession,
    toggleArchivedSessions,
    restoreSession,
    deleteArchivedSession,
  } = useArchiveActions({
    sessionRuntime,
    safeSetError,
    setConfirmState,
    refresh,
    refreshArchived,
    refreshSessions,
    removeAgentSessions,
    selectedRef,
    activeSessionRef,
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
    invalidateNavigation: navigation.invalidateNavigation,
  });

  const selectAgent = (agentName: string) => {
    // 切换不被任何运行态拦住：每条会话是独立的一路，切走不影响它继续跑
    //（ChatView 卸载也不中止）。这里只做「选中 + 收起会话视图」。
    const previous = selectedRef.current;
    navigation.invalidateNavigation();
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
    navigation.invalidateNavigation();
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

  const current = agents.find((a) => a.name === selected) ?? null;

  const regularAgents = agents.filter((agent) => !agent.subagent);
  const subagents = agents.filter((agent) => agent.subagent);
  const sectionAgents = navSection === "subs" ? subagents : regularAgents;
  const sectionArchived = archivedAgents.filter(
    (agent) => agent.subagent === (navSection === "subs"),
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
        <Rail
          navSection={navSection}
          settingsOpen={settingsOpen}
          onOpenSection={openSection}
          onToggleSettings={() => {
            setSidebarOpen(false);
            // 再点一次即返回设置前的界面（与返回按钮同义）
            setSettingsOpen((open) => !open);
          }}
        />

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
              onOpenSession={(agentName, sessionId) =>
                void navigation.openSession(agentName, sessionId)}
            />
          ) : navSection === "skills" ? (
            <SkillsSidebar
              library={skillLibrary}
              selected={skillSelected}
              error={skillLoadError}
              onSelect={setSkillSelected}
              onRefresh={() => void loadSkillLibrary()}
            />
          ) : (
            <Sidebar
              navSection={navSection}
              agents={sectionAgents}
              archivedAgents={sectionArchived}
              selected={selected}
              creating={creating}
              chatOpen={chatOpen}
              viewSessionId={viewSessionId}
              activeSession={activeSession}
              expandedAgents={expandedAgents}
              archivedSessions={archivedSessions}
              archivedSessionsExpanded={archivedSessionsExpanded}
              archivedOpen={archivedOpen}
              sessionsFor={sessionsFor}
              agentHasRunning={agentHasRunning}
              isConversationRunning={isConversationRunning}
              onCreateAgent={startCreating}
              onSelectAgent={selectAgent}
              onStartNewSession={navigation.startNewSession}
              onOpenSession={(agentName, sessionId) =>
                void navigation.openSession(agentName, sessionId)}
              onToggleAgentExpanded={(agentName, expanded) =>
                setExpandedAgents((previous) => ({ ...previous, [agentName]: expanded }))}
              onArchiveAgent={(name) => void archiveAgent(name)}
              onDeleteAgent={(name, archived) => void deleteAgent(name, archived)}
              onRestoreAgent={(name) => void restoreAgent(name)}
              onArchiveSession={(agentName, sessionId) => void archiveSession(agentName, sessionId)}
              onDeleteSession={(agentName, sessionId) => void deleteSession(agentName, sessionId)}
              onToggleArchivedSessions={(agentName) => void toggleArchivedSessions(agentName)}
              onRestoreSession={(agentName, sessionId) => void restoreSession(agentName, sessionId)}
              onDeleteArchivedSession={(agentName, sessionId) =>
                void deleteArchivedSession(agentName, sessionId)}
              onToggleArchivedOpen={() => setArchivedOpen((open) => !open)}
            />
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
              {settingsOpen ? "设置" : creating ? "新建 Agent" : navSection === "skills" ? "技能库" : current?.name ?? "Pipi"}
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
          ) : navSection === "skills" ? (
            <SkillsView
              library={skillLibrary}
              loadError={skillLoadError}
              selected={skillSelected}
              onSelect={setSkillSelected}
              onRefresh={loadSkillLibrary}
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
                  blockedSessionIds={[...navigation.blockedSessionIdsRef.current]}
                  onBack={() => {
                    navigation.invalidateNavigation();
                    // 释放本 Agent 空闲的会话（否则归档/删除会被占用检查拒），
                    // 正在跑的那条留着：返回后它继续跑（卸载不再中止运行）。
                    void invoke("new_session", { agentName: current.name }).catch(() => {});
                    setChatOpen(false);
                    setSidebarOpen(false);
                  }}
                  onError={safeSetError}
                  onNewSession={navigation.createSessionFromChat}
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
                  blockedSessionIds={[...navigation.blockedSessionIdsRef.current]}
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

      <StatusBar
        connection={connection}
        agentCount={agents.length}
        sessionCount={sessionTotal}
        runningCount={runningSessions.size}
        version={APP_VERSION}
      />

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
