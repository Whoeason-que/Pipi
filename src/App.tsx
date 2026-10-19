import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import AgentWorkbench from "./agents/AgentWorkbench";
import CreateAgentForm from "./agents/CreateAgentForm";
import CommandPalette from "./app/CommandPalette";
import { LayoutContext } from "./app/layout-context";
import { agentSection, agentsInSection, quickItems, type AgentSection, type AppView, type NavSection, type QuickTarget } from "./app/navigation";
import WorkspaceOverview from "./app/WorkspaceOverview";
import AppNavigation, { NavigationToggle } from "./app/AppNavigation";
import AgentSidebar from "./app/AgentSidebar";
import { useArchiveActions, type ConfirmRequest } from "./app/use-archive-actions";
import { useHostEvents } from "./app/use-host-events";
import { useSessionList } from "./app/use-session-list";
import { useSessionNavigation } from "./app/use-session-navigation";
import { useSettings } from "./app/use-settings";
import ChatView from "./chat/ChatView";
import { ForumSidebar } from "./forum/ForumSidebar";
import ForumView from "./forum/ForumView";
import { formatRuntimeError, isAuthSilentError } from "./chat-runtime";
import ConfirmModal from "./components/ConfirmModal";
import { useDialogFocus } from "./components/use-dialog-focus";
import { useI18n } from "./i18n";
import { IconAlertTriangle, IconClose, IconPanel, IconSearch } from "./icons";
import Login from "./Login";
import { getAuthStatus, invoke, isTauriRuntime, listen, onAuthRequired } from "./platform";
import { useResizableWidth } from "./resizable";
import { SearchPanel } from "./SearchPanel";
import { SessionRuntime } from "./session-runtime";
import SettingsView from "./settings/SettingsView";
import SettingsSidebar from "./settings/SettingsSidebar";
import type { SettingsSection } from "./settings/settings-sections";
import { SkillsSidebar } from "./skills/SkillsSidebar";
import { SkillsView } from "./skills/SkillsView";
import { applyTheme } from "./theme";
import type { SessionInfoView, SessionSummaryView, SkillLibrary } from "./types";

export default function App() {
  const { t } = useI18n();
  const [sessionRuntime] = useState(() => new SessionRuntime({ invoke, listen }));
  const [view, setView] = useState<AppView>({ kind: "home" });
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("appearance");
  const viewRef = useRef(view);
  const previousView = useRef<AppView>({ kind: "home" });
  const [authStatus, setAuthStatus] = useState({ authRequired: false, authenticated: true, checking: !isTauriRuntime() });
  const [navSection, setNavSection] = useState<NavSection>("agents");
  const [expandedAgents, setExpandedAgents] = useState<Record<string, boolean>>({});
  const [archivedOpen, setArchivedOpen] = useState(false);
  const [archivedSessions, setArchivedSessions] = useState<Record<string, SessionSummaryView[]>>({});
  const [archivedSessionsExpanded, setArchivedSessionsExpanded] = useState<Record<string, boolean>>({});
  const [chatKey, setChatKey] = useState(0);
  const [activeSession, setActiveSession] = useState<SessionInfoView | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [mobile, setMobile] = useState(() => window.matchMedia("(max-width: 860px)").matches);
  const sidebarRef = useRef<HTMLElement>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try { return localStorage.getItem("pipi-sidebar-collapsed") === "true"; } catch { return false; }
  });
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [skillLibrary, setSkillLibrary] = useState<SkillLibrary | null>(null);
  const [skillSelected, setSkillSelected] = useState<string | null>(null);
  const [skillLoadError, setSkillLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmState, setConfirmState] = useState<ConfirmRequest | null>(null);
  const selected = "agentName" in view ? view.agentName : null;
  const selectedRef = useRef<string | null>(selected);
  const activeSessionRef = useRef(activeSession);
  viewRef.current = view;
  selectedRef.current = selected;
  activeSessionRef.current = activeSession;
  const sidebarResize = useResizableWidth({ storageKey: "sidebar-width", initial: 272, min: 200, max: 460, edge: "right" });

  const commitView = useCallback((next: AppView) => {
    viewRef.current = next;
    selectedRef.current = "agentName" in next ? next.agentName : null;
    setView(next);
    setSidebarOpen(false);
  }, []);
  const safeSetError = useCallback((message: string | null) => {
    if (message?.includes("需要 PIPI_AUTH_TOKEN")) return;
    if (message?.includes("当前会话仍在运行")) message = "当前会话正在运行，请先停止或使用插话继续沟通";
    if (message?.includes("正在运行或有托管后台任务")) message = "会话仍在运行或有后台任务，请先停止再操作";
    setError(message);
  }, []);
  const { agents, archivedAgents, sessionsByAgent, refresh, refreshSessions, refreshArchived,
    removeAgentSessions, sessionsFor } = useSessionList(safeSetError);
  const current = agents.find(agent => agent.name === selected);
  const collection: AgentSection = view.kind === "home" ? view.section ?? "agents"
    : current ? agentSection(current) : navSection === "subagents" ? "subagents" : "agents";
  const { settings, setSettings, updateSettings } = useSettings(setError);
  const onOpenChat = useCallback((agentName: string, sessionId: string | null) => {
    setNavSection(agentSection(agents.find(agent => agent.name === agentName)));
    commitView({ kind: "chat", agentName, sessionId });
  }, [commitView, agents]);
  const navigation = useSessionNavigation({ setError, selectedRef, activeSessionRef, onOpenChat,
    setSidebarOpen, setExpandedAgents, setActiveSession, setChatKey });
  const { runningSessions, handleRunningChange, isConversationRunning, agentHasRunning,
    forgetRunningSession, forgetRunningAgent } = useHostEvents({ sessionRuntime, refreshSessions,
    safeSetError, setError, setActiveSession, selectedRef, activeSessionRef,
    blockedSessionIdsRef: navigation.blockedSessionIdsRef, sessionInfoRequestRef: navigation.sessionInfoRequestRef });

  const navigate = (next: AppView) => {
    if (next.kind === "home" && !next.section) next = { ...next, section: collection };
    if (next.kind === "settings" && viewRef.current.kind !== "settings") {
      previousView.current = viewRef.current;
      setSettingsSection("appearance");
      setSidebarCollapsed(false);
    }
    navigation.invalidateNavigation();
    const previous = viewRef.current;
    // 释放离开视图的空闲宿主槽；运行中的会话由核心保留，视图卸载不会停止它。
    if ((previous.kind === "chat" || previous.kind === "workbench") && (next.kind !== previous.kind
      || !("agentName" in next) || next.agentName !== previous.agentName)) {
      void invoke("new_session", { agentName: previous.agentName }).catch(() => {});
    }
    if (next.kind === "skills") setNavSection("skills");
    else if (next.kind === "forum") setNavSection("forum");
    else if (next.kind === "home") setNavSection(next.section ?? "agents");
    else if (next.kind === "create") setNavSection("agents");
    else if ("agentName" in next) setNavSection(agentSection(agents.find(agent => agent.name === next.agentName)));
    commitView(next);
  };
  const archive = useArchiveActions({ sessionRuntime, safeSetError, setConfirmState, refresh, refreshArchived,
    refreshSessions, removeAgentSessions, selectedRef, activeSessionRef, viewRef, onNavigate: navigate,
    setActiveSession, forgetRunningSession, forgetRunningAgent, archivedSessionsExpanded,
    setArchivedSessions, setArchivedSessionsExpanded, invalidateNavigation: navigation.invalidateNavigation });
  const selectAgent = (agentName: string) => navigate({ kind: "agent", agentName });
  const configureAgent = (agentName: string) => navigate({ kind: "workbench", agentName });
  const startCreating = () => navigate({ kind: "create" });
  const openSession = (agentName: string, sessionId: string) => { void navigation.openSession(agentName, sessionId); };
  const closePalette = useCallback(() => setPaletteOpen(false), []);
  const closeNavigation = useCallback(() => setSidebarOpen(false), []);
  useDialogFocus(sidebarRef, closeNavigation, mobile && sidebarOpen);
  const toggleNavigation = useCallback(() => {
    if (window.matchMedia("(max-width: 860px)").matches) setSidebarOpen(open => !open);
    else setSidebarCollapsed(collapsed => !collapsed);
  }, []);
  const layout = useMemo(() => ({ navigationOpen: sidebarOpen, closeNavigation, toggleNavigation }), [sidebarOpen, closeNavigation, toggleNavigation]);

  useEffect(() => { try { localStorage.setItem("pipi-sidebar-collapsed", String(sidebarCollapsed)); } catch { /* 内存状态仍有效 */ } }, [sidebarCollapsed]);
  useEffect(() => {
    const query = window.matchMedia("(max-width: 860px)");
    const change = () => { setMobile(query.matches); setSidebarOpen(false); };
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.isComposing || authStatus.checking || !authStatus.authenticated) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k"
        && !document.querySelector('[aria-modal="true"]')) {
        event.preventDefault(); setPaletteOpen(open => !open);
      }
      if (event.key === "Escape" && !document.querySelector('[aria-modal="true"]')) setSidebarOpen(false);
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [authStatus]);
  useEffect(() => {
    let active = true;
    if (!isTauriRuntime()) void getAuthStatus().then(status => {
      if (active) setAuthStatus({ ...status, checking: false });
    });
    const unsubscribe = onAuthRequired(() => setAuthStatus(previous => ({ ...previous, authenticated: false })));
    return () => { active = false; unsubscribe(); };
  }, []);
  useEffect(() => {
    if (authStatus.checking || (authStatus.authRequired && !authStatus.authenticated)) return;
    void refresh(); void refreshArchived();
    let active = true;
    invoke("get_settings").then(next => { if (active) { setSettings(next); applyTheme(next.theme); } })
      .catch(value => { if (active && !isAuthSilentError(value)) safeSetError(formatRuntimeError(value)); });
    return () => { active = false; };
  }, [authStatus, refresh, refreshArchived, safeSetError, setSettings]);
  const loadSkillLibrary = useCallback(async (): Promise<SkillLibrary | null> => {
    try { const library = await invoke("skill_library"); setSkillLibrary(library); setSkillLoadError(null); return library; }
    catch (value) { setSkillLoadError(formatRuntimeError(value)); return null; }
  }, []);
  useEffect(() => { if (view.kind === "skills") void loadSkillLibrary(); }, [view.kind, loadSkillLibrary]);
  useEffect(() => {
    if (skillSelected && skillLibrary && !skillLibrary.skills.some(skill => skill.name === skillSelected)) setSkillSelected(null);
  }, [skillLibrary, skillSelected]);

  const toggleSettings = () => {
    if (view.kind === "settings") navigate(previousView.current);
    else navigate({ kind: "settings" });
  };
  const chooseQuick = (target: QuickTarget) => {
    if (target.kind === "agent") selectAgent(target.agentName);
    else if (target.kind === "chat") openSession(target.agentName, target.sessionId);
    else if (target.action === "new-session") { if (selected) void navigation.startNewSession(selected); }
    else navigate({ kind: target.action });
  };
  const visibleAgents = agentsInSection(agents, collection);
  const visibleArchivedAgents = agentsInSection(archivedAgents, collection);
  useEffect(() => {
    if (current && (view.kind === "agent" || view.kind === "chat" || view.kind === "workbench")) setNavSection(agentSection(current));
  }, [current?.name, current?.subagent, view.kind]);
  const title = view.kind === "home" ? t(collection === "subagents" ? "nav.subagents" : "nav.workspace") : view.kind === "settings" ? t("nav.settings") : view.kind === "create" ? t("nav.newAgent")
    : view.kind === "skills" ? t("nav.skills") : view.kind === "forum" ? "论坛" : current?.name ?? "Pipi";
  const sessionTitle = view.kind === "chat" ? (sessionsByAgent[view.agentName] ?? []).find(session => session.id === view.sessionId)?.title : undefined;
  const overviewProps = { section: collection, agents: visibleAgents, sessions: sessionsByAgent, running: runningSessions, onAgent: selectAgent, onSession: openSession,
    onNewSession: (name: string) => { void navigation.startNewSession(name); }, onConfigure: configureAgent, onCreate: startCreating };

  if (authStatus.checking) return <div className="login-container" />;
  if (authStatus.authRequired && !authStatus.authenticated) return <Login onSuccess={() => {
    setAuthStatus(previous => ({ ...previous, authenticated: true })); safeSetError(null);
  }} />;

  return <LayoutContext.Provider value={layout}><div className={`app${sidebarOpen ? " sidebar-open" : ""}${sidebarCollapsed ? " sidebar-collapsed" : ""}`}
    style={{ "--sidebar-width": `${sidebarResize.width}px` } as React.CSSProperties}>
    <div className="app-body">
      <AppNavigation navSection={navSection} settingsOpen={view.kind === "settings"} onToggleSettings={toggleSettings}
        running={{ agents: agentsInSection(agents, "agents").some(agent => agentHasRunning(agent.name)),
          subagents: agentsInSection(agents, "subagents").some(agent => agentHasRunning(agent.name)) }}
        onOpenSection={section => {
          if (section === "skills") navigate({ kind: "skills" });
          else if (section === "forum") navigate({ kind: "forum" });
          else if (section === "agents" || section === "subagents") navigate({ kind: "home", section });
          setNavSection(section); setSidebarCollapsed(false); setSidebarOpen(true);
        }} />
      <aside ref={sidebarRef} inert={mobile && !sidebarOpen} className="sidebar" id="primary-navigation" aria-label={view.kind === "settings" ? t("settings.title") : "Agent 与会话列表"}>
        <div className="sb-head"><button type="button" className="sidebar-home" aria-label={t("nav.home")} onClick={() => navigate({ kind: "home" })}><b>Pipi</b><span>{t("nav.workspace")}</span></button>
          <span className="spacer" /><button type="button" className="icon-btn" title={t("nav.collapseSidebar")} aria-label={t("nav.collapseSidebar")} onClick={toggleNavigation}><IconPanel /></button></div>
        <button type="button" className="sidebar-quick" onClick={() => setPaletteOpen(true)}><IconSearch /><span>{t("nav.quickSearch")}</span><kbd>⌘ K</kbd></button>
        {view.kind === "settings" ? settings && <SettingsSidebar settings={settings} section={settingsSection}
          showLogout={!isTauriRuntime() && authStatus.authRequired}
          onSelect={section => { setSettingsSection(section); setSidebarOpen(false); }} />
          : navSection === "search" ? <SearchPanel query={searchQuery} onQueryChange={setSearchQuery} agents={agents}
          sessionsByAgent={sessionsByAgent} isRunning={isConversationRunning} onSelectAgent={selectAgent} onOpenSession={openSession} />
          : navSection === "skills" ? <SkillsSidebar library={skillLibrary} selected={skillSelected} error={skillLoadError}
            onSelect={name => { setSkillSelected(name); setSidebarOpen(false); }} onRefresh={() => void loadSkillLibrary()} />
          : navSection === "forum" ? <ForumSidebar />
          : <AgentSidebar key={collection} section={collection} agents={visibleAgents} archivedAgents={visibleArchivedAgents} selected={selected} chatOpen={view.kind === "chat"}
            viewSessionId={view.kind === "chat" ? view.sessionId : null} expandedAgents={expandedAgents}
            archivedSessions={archivedSessions} archivedSessionsExpanded={archivedSessionsExpanded} archivedOpen={archivedOpen}
            sessionsFor={sessionsFor} agentHasRunning={agentHasRunning} isConversationRunning={isConversationRunning}
            onCreateAgent={startCreating} onSelectAgent={selectAgent} onConfigure={configureAgent}
            onStartNewSession={name => void navigation.startNewSession(name)} onOpenSession={openSession}
            onToggleAgentExpanded={(name, expanded) => setExpandedAgents(previous => ({ ...previous, [name]: expanded }))}
            onArchiveAgent={name => void archive.archiveAgent(name)} onDeleteAgent={(name, archived) => void archive.deleteAgent(name, archived)}
            onRestoreAgent={name => void archive.restoreAgent(name)} onArchiveSession={(name, id) => void archive.archiveSession(name, id)}
            onDeleteSession={(name, id) => void archive.deleteSession(name, id)} onToggleArchivedSessions={name => void archive.toggleArchivedSessions(name)}
            onRestoreSession={(name, id) => void archive.restoreSession(name, id)} onDeleteArchivedSession={(name, id) => void archive.deleteArchivedSession(name, id)}
            onToggleArchivedOpen={() => setArchivedOpen(open => !open)} />}
        <div className="sidebar-footer"><span><i className="activity-dot" />{t("workspace.localWorkspace")}</span><span>v{__PIPI_VERSION__}</span></div>
      </aside>
      <div className="resize-handle sidebar-resize" title="拖动调整侧栏宽度" aria-label="调整侧栏宽度" {...sidebarResize.handleProps} />
      {sidebarOpen && <button type="button" className="sidebar-backdrop" aria-label="关闭导航" onClick={closeNavigation} />}
      <main className="main" inert={mobile && sidebarOpen}>
        {(view.kind === "home" || view.kind === "agent" || (!current && (view.kind === "chat" || view.kind === "workbench"))) && <header className="workspace-toolbar">
          <NavigationToggle />
          <span className="toolbar-divider" /><span>{title}</span><span className="spacer" />
          <button type="button" className="icon-btn" aria-label="打开快捷操作" title="快捷操作 Ctrl/⌘ K" onClick={() => setPaletteOpen(true)}><IconSearch /></button>
        </header>}
        {error && (
          <div className="error" role="alert">
            <IconAlertTriangle size={15} />
            <span>{error}</span>
            <button type="button" className="error-dismiss" aria-label="关闭错误提示" onClick={() => setError(null)}>
              <IconClose />
            </button>
          </div>
        )}
        {view.kind === "settings" && settings ? <SettingsView settings={settings} section={settingsSection} onChange={updateSettings} onClose={toggleSettings}
          showLogout={!isTauriRuntime() && authStatus.authRequired} />
          : view.kind === "create" ? <CreateAgentForm providers={settings?.providers ?? []} defaultProviderId={settings?.defaultProviderId ?? null}
            onCreated={async name => { await refresh(); selectAgent(name); }} onCancel={() => navigate({ kind: "home" })} onError={safeSetError} />
          : view.kind === "skills" ? <SkillsView library={skillLibrary} loadError={skillLoadError} selected={skillSelected}
            onSelect={setSkillSelected} onRefresh={loadSkillLibrary} onError={safeSetError} />
          : view.kind === "forum" ? <ForumView />
          : current && view.kind === "chat" && settings ? <ChatView key={`${current.name}-${chatKey}`} agent={current} providers={settings.providers}
            title={sessionTitle || t("chat.newSessionTitle")} sessionId={view.sessionId} sessions={sessionRuntime}
            blockedSessionIds={[...navigation.blockedSessionIdsRef.current]} onBack={() => selectAgent(current.name)} onError={safeSetError}
            onNewSession={navigation.createSessionFromChat} onRunningChange={handleRunningChange} onSessionReset={info => {
              setActiveSession(info ?? null);
              const active = viewRef.current;
              if (info && active.kind === "chat" && active.agentName === current.name) commitView({ ...active, sessionId: info.sessionId });
              void refreshSessions([current.name]);
              if (archivedSessionsExpanded[current.name]) void invoke("list_sessions", { agentName: current.name, archived: true })
                .then(list => setArchivedSessions(previous => ({ ...previous, [current.name]: list }))).catch(() => {});
            }} />
          : current && view.kind === "workbench" && settings ? <AgentWorkbench key={current.name} agent={current} providers={settings.providers}
            onSaved={async () => { await refresh(); }} onBack={() => selectAgent(current.name)} onError={safeSetError}
            blockedSessionIds={[...navigation.blockedSessionIdsRef.current]} sessions={sessionRuntime} onRunningChange={handleRunningChange} />
          : <WorkspaceOverview {...overviewProps} agent={view.kind === "agent" ? current : undefined} />}
      </main>
    </div>
    {paletteOpen && <CommandPalette items={quickItems(agents, sessionsByAgent, selected)} onChoose={chooseQuick} onClose={closePalette} />}
    {confirmState && <ConfirmModal title={confirmState.title} message={confirmState.message} onConfirm={confirmState.action} onCancel={() => setConfirmState(null)} />}
  </div></LayoutContext.Provider>;
}
