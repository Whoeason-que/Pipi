import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import AgentWorkbench from "./agents/AgentWorkbench";
import CreateAgentForm from "./agents/CreateAgentForm";
import CommandPalette from "./app/CommandPalette";
import { LayoutContext } from "./app/layout-context";
import { agentSection, agentsInSection, quickItems, type AgentSection, type AppView, type NavSection, type QuickTarget } from "./app/navigation";
import { NavigationMemory } from "./app/navigation-memory";
import WorkspaceOverview from "./app/WorkspaceOverview";
import AppNavigation, { NavigationToggle } from "./app/AppNavigation";
import AgentSidebar from "./app/AgentSidebar";
import { useArchiveActions, type ConfirmRequest } from "./app/use-archive-actions";
import { useHostEvents } from "./app/use-host-events";
import { readStored, writeStored } from "./storage.ts";
import { useNotificationNavigation } from "./app/use-notification-navigation";
import { useSessionList } from "./app/use-session-list";
import { useSessionNavigation } from "./app/use-session-navigation";
import { useSettings } from "./app/use-settings";
import ChatView from "./chat/ChatView";
import { WorkspaceSidebar } from "./files/WorkspaceSidebar";
import { WorkspaceView } from "./files/WorkspaceView";
import { ForumSidebar } from "./forum/ForumSidebar";
import ForumView from "./forum/ForumView";
import { ForumProvider } from "./forum/forum-state";
import {
  getWorkspaceBasename,
  normalizeWorkspacePath,
} from "./app/workspaces";
import { useWorkspaceFilter } from "./app/use-workspace-filter";
import { useFilesBridge } from "./app/use-files-bridge";
import WorkspaceFilterBar from "./app/WorkspaceFilterBar";
import { formatRuntimeError, isAuthSilentError } from "./chat-runtime";
import ConfirmModal from "./components/ConfirmModal";
import { useDialogFocus } from "./components/use-dialog-focus";
import { useI18n } from "./i18n";
import { IconAlertTriangle, IconClose, IconSearch } from "./icons";
import Login from "./Login";
import { getAuthStatus, invoke, isTauriRuntime, listen, onAuthRequired } from "./platform";
import { useResizableWidth } from "./resizable";
import { SearchPanel } from "./SearchPanel";
import { SessionRuntime } from "./session-runtime";
import type { SessionController } from "./session-controller";
import SettingsView from "./settings/SettingsView";
import SettingsSidebar from "./settings/SettingsSidebar";
import type { SettingsSection } from "./settings/settings-sections";
import { SkillsSidebar } from "./skills/SkillsSidebar";
import { StatsSidebar } from "./stats/StatsSidebar";
import StatsView from "./stats/StatsView";
import { SkillsView } from "./skills/SkillsView";
import { applyTheme } from "./theme";
import type { AgentDefinition, SessionInfoView, SessionSummaryView, SkillLibrary } from "./types";

export default function App() {
  const { t } = useI18n();
  const [sessionRuntime] = useState(() => new SessionRuntime({ invoke, listen }));
  const [navigationMemory] = useState(() => new NavigationMemory());
  const agentsRef = useRef<AgentDefinition[]>([]);
  const [view, setView] = useState<AppView>({ kind: "home" });
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("appearance");
  const viewRef = useRef(view);
  /** 此刻在看的会话（chat 视图）；完成时用户没在看就记未读。 */
  const viewedSessionRef = useRef<{ agentName: string; sessionId: string | null } | null>(null);
  const previousView = useRef<AppView>({ kind: "home" });
  const [authStatus, setAuthStatus] = useState({ authRequired: false, authenticated: true, checking: !isTauriRuntime() });
  const [navSection, setNavSection] = useState<NavSection>("agents");
  const [archivedOpen, setArchivedOpen] = useState(false);
  const [archivedSessions, setArchivedSessions] = useState<Record<string, SessionSummaryView[]>>({});
  const [archivedSessionsExpanded, setArchivedSessionsExpanded] = useState<Record<string, boolean>>({});
  const [chatKey, setChatKey] = useState(0);
  const [activeSession, setActiveSession] = useState<SessionInfoView | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [mobile, setMobile] = useState(() => window.matchMedia("(max-width: 860px)").matches);
  const sidebarRef = useRef<HTMLElement>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => readStored("pipi-sidebar-collapsed", "false") === "true",
  );
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [forumVisited, setForumVisited] = useState(false);
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
  viewedSessionRef.current = view.kind === "chat"
    ? { agentName: view.agentName, sessionId: view.sessionId }
    : null;
  const sidebarResize = useResizableWidth({ storageKey: "sidebar-width", initial: 248, min: 200, max: 460, edge: "right" });

  const commitView = useCallback((next: AppView, remember = true, options?: { keepSidebar?: boolean }) => {
    viewRef.current = next;
    selectedRef.current = "agentName" in next ? next.agentName : null;
    if (remember) navigationMemory.remember(next, agentsRef.current);
    setView(next);
    if (!options?.keepSidebar) setSidebarOpen(false);
  }, [navigationMemory]);
  const safeSetError = useCallback((message: string | null) => {
    if (message?.includes("需要 PIPI_AUTH_TOKEN") || message?.includes("PIPI_AUTH_TOKEN")) return;
    if (message?.includes("当前会话仍在运行") || message?.includes("is running")) message = t("app.sessionRunningError");
    if (message?.includes("正在运行或有托管后台任务") || message?.includes("background tasks")) message = t("app.backgroundTaskError");
    setError(message);
  }, [t]);
  const { agents, archivedAgents, sessionsByAgent, refresh, refreshSessions, refreshArchived,
    removeAgentSessions, sessionsFor } = useSessionList(safeSetError);
  agentsRef.current = agents;

  useEffect(() => sessionRuntime.subscribeRekeys((controller, previous, next, temporary) => {
    if (temporary || !navigationMemory.rekey(controller.agentName, previous, next, controller)) return;
    const saved = previousView.current;
    if (saved.kind === "chat" && saved.agentName === controller.agentName && saved.sessionId === previous) {
      previousView.current = { ...saved, sessionId: next };
    }
  }), [navigationMemory, sessionRuntime]);

  const {
    activeWorkspace,
    workspaces,
    workspaceGroups,
    filteredAgents,
    filteredArchivedAgents,
    filteredSessionsByAgent,
    select: selectWorkspacePath,
    removeCustom: removeCustomWorkspaceEntry,
  } = useWorkspaceFilter({ agents, archivedAgents, sessionsByAgent });
  const statsAgents = useMemo(
    () => filteredAgents.filter((agent) => !agent.subagent),
    [filteredAgents],
  );
  const current = filteredAgents.find(agent => agent.name === selected);
  const collection: AgentSection = view.kind === "home" ? view.section ?? "agents"
    : current ? agentSection(current) : navSection === "subagents" ? "subagents" : "agents";
  const { settings, setSettings, updateSettings } = useSettings(setError);
  const onOpenChat = useCallback((agentName: string, sessionId: string | null, temporary = false) => {
    setNavSection(agentSection(agents.find(agent => agent.name === agentName)));
    if (!temporary && sessionId === null) navigationMemory.unbindDraft(agentName);
    commitView(temporary ? { kind: "workbench", agentName } : { kind: "chat", agentName, sessionId });
  }, [commitView, agents, navigationMemory]);
  const navigation = useSessionNavigation({ setError, activeSessionRef, onOpenChat,
    setSidebarOpen, setActiveSession, setChatKey });
  useNotificationNavigation(!authStatus.checking && authStatus.authenticated && agents.length > 0, async (agent, session) => {
    selectWorkspacePath("all");
    await navigation.openSession(agent, session);
  });
  const { runningSessions, handleRunningChange, isConversationRunning, agentHasRunning,
    sessionStatus, sessionUnread, markSessionRead, markSessionInterrupted,
    forgetRunningSession, forgetRunningAgent } = useHostEvents({ sessionRuntime, refreshSessions,
    safeSetError, setError, setActiveSession, selectedRef, activeSessionRef, viewedSessionRef,
    blockedSessionIdsRef: navigation.blockedSessionIdsRef, sessionInfoRequestRef: navigation.sessionInfoRequestRef });

  // 打开会话即视为已读：未读蓝点只在离开期间完成时保留。
  const viewedAgent = view.kind === "chat" ? view.agentName : null;
  const viewedSessionId = view.kind === "chat" ? view.sessionId : null;
  useEffect(() => {
    if (viewedAgent !== null && viewedSessionId !== null) markSessionRead(viewedAgent, viewedSessionId);
  }, [viewedAgent, viewedSessionId, markSessionRead]);

  const navigate = (next: AppView, remember = true, options?: { keepSidebar?: boolean }) => {
    if (next.kind === "home" && !next.section) next = { ...next, section: collection };
    if (next.kind === "settings" && viewRef.current.kind !== "settings") {
      previousView.current = viewRef.current;
      setSidebarCollapsed(false);
    }
    if (next.kind === "forum") setForumVisited(true);
    navigation.invalidateNavigation();
    const previous = viewRef.current;
    // 释放离开视图的空闲宿主槽；运行中的会话由核心保留，视图卸载不会停止它。
    if ((previous.kind === "chat" || previous.kind === "workbench") && (next.kind !== previous.kind
      || !("agentName" in next) || next.agentName !== previous.agentName)) {
      void navigation.releaseIdle(previous.agentName);
    }
    if (next.kind === "files" || next.kind === "git") setNavSection(next.kind);
    else if (next.kind === "skills") setNavSection("skills");
    else if (next.kind === "forum") setNavSection("forum");
    else if (next.kind === "home") setNavSection(next.section ?? "agents");
    else if (next.kind === "create") setNavSection("agents");
    else if (next.kind === "stats") setNavSection("stats");
    else if ("agentName" in next) setNavSection(agentSection(agents.find(agent => agent.name === next.agentName)));
    commitView(next, remember, options);
  };

  const handleSelectWorkspace = (nextWorkspace: string) => {
    selectWorkspacePath(nextWorkspace);

    const target = normalizeWorkspacePath(nextWorkspace);
    const nextFiltered = nextWorkspace === "all"
      ? agents
      : agents.filter((a) => a.subagent || normalizeWorkspacePath(a.workspace) === target);

    if (selected && !nextFiltered.some((a) => a.name === selected)) {
      const fallback = nextFiltered[0]?.name ?? null;
      if (view.kind === "chat" || view.kind === "workbench" || view.kind === "agent") {
        // 工作区过滤暂时隐藏 Agent，不抹掉返回原工作区时要恢复的页面。
        navigate({ kind: "home", section: collection }, false);
      } else if (view.kind === "files" || view.kind === "git") {
        navigate({ kind: view.kind, agentName: fallback });
      } else if (view.kind === "stats") {
        navigate({
          kind: "stats",
          agentName: nextFiltered.find((agent) => !agent.subagent)?.name ?? null,
        });
      }
    }
  };

  const handleRemoveCustomWorkspace = (path: string) => {
    removeCustomWorkspaceEntry(path);
    if (normalizeWorkspacePath(activeWorkspace) === normalizeWorkspacePath(path)) {
      handleSelectWorkspace("all");
    }
  };
  const archive = useArchiveActions({ sessionRuntime, safeSetError, setConfirmState, refresh, refreshArchived,
    refreshSessions, removeAgentSessions, selectedRef, activeSessionRef, viewRef, onNavigate: navigate,
    setActiveSession, forgetRunningSession, forgetRunningAgent,
    forgetNavigationSession: (agentName, sessionId) => {
      navigationMemory.forgetSession(agentName, sessionId);
      const saved = previousView.current;
      if (saved.kind === "chat" && saved.agentName === agentName && saved.sessionId === sessionId) previousView.current = { kind: "agent", agentName };
    },
    forgetNavigationAgent: agentName => {
      navigationMemory.forgetAgent(agentName);
      const saved = previousView.current;
      if ("agentName" in saved && saved.agentName === agentName) previousView.current = { kind: "home" };
    }, archivedSessionsExpanded,
    setArchivedSessions, setArchivedSessionsExpanded, invalidateNavigation: navigation.invalidateNavigation });
  const selectAgent = (agentName: string) => navigate({ kind: "agent", agentName });
  const configureAgent = (agentName: string) => navigate({ kind: "workbench", agentName });
  const startCreating = () => navigate({ kind: "create" });
  const openSession = (agentName: string, sessionId: string) => { void navigation.openSession(agentName, sessionId); };
  // 左栏的会话仓库行：默认打开最近活跃的分支（list_sessions 已按 last_active 降序）。
  const openRepo = (agentName: string) => {
    const latest = sessionsByAgent[agentName]?.[0];
    if (latest) openSession(agentName, latest.id);
    else selectAgent(agentName);
  };
  const closePalette = useCallback(() => setPaletteOpen(false), []);
  const closeNavigation = useCallback(() => setSidebarOpen(false), []);
  useDialogFocus(sidebarRef, closeNavigation, mobile && sidebarOpen);
  const toggleNavigation = useCallback(() => {
    if (window.matchMedia("(max-width: 860px)").matches) setSidebarOpen(open => !open);
    else setSidebarCollapsed(collapsed => !collapsed);
  }, []);
  const layout = useMemo(() => ({ navigationOpen: sidebarOpen, closeNavigation, toggleNavigation }), [sidebarOpen, closeNavigation, toggleNavigation]);

  useEffect(() => { writeStored("pipi-sidebar-collapsed", String(sidebarCollapsed)); }, [sidebarCollapsed]);
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
    // 设置分区同样把它的分区列表放在抽屉里：窄屏开着抽屉点齿轮时不要把它收起。
    const navOptions = { keepSidebar: mobile && sidebarOpen };
    if (view.kind !== "settings") {
      navigate({ kind: "settings" }, true, navOptions);
      return;
    }
    const previous = previousView.current;
    if (previous.kind === "chat" && previous.sessionId !== null) {
      void navigation.openSession(previous.agentName, previous.sessionId, navOptions);
    } else {
      navigate(previous, true, navOptions);
    }
  };
  const chooseQuick = (target: QuickTarget) => {
    if (target.kind === "agent") selectAgent(target.agentName);
    else if (target.kind === "chat") openSession(target.agentName, target.sessionId);
    else if (target.action === "new-session") { if (selected) void navigation.startNewSession(selected); }
    else if (target.action === "files" || target.action === "git" || target.action === "stats") openSection(target.action);
    else navigate({ kind: target.action });
  };
  const visibleAgents = agentsInSection(filteredAgents, collection);
  const visibleArchivedAgents = agentsInSection(filteredArchivedAgents, collection);
  useEffect(() => {
    if (current && (view.kind === "agent" || view.kind === "chat" || view.kind === "workbench")) setNavSection(agentSection(current));
  }, [current?.name, current?.subagent, view.kind]);

  const files = useFilesBridge({
    agents, filteredAgents, sessionsByAgent, activeWorkspace, workspaces,
    current: current ?? null,
    viewKind: view.kind,
    authenticated: !authStatus.checking && authStatus.authenticated,
    agentRunning: current ? agentHasRunning(current.name) : false,
    navigate, sessionRuntime, navigation,
  });
  const { previewWorkspacePaths, setPreviewWorkspacePaths, currentWorkspaceTarget } = files;
  const openSection = (section: NavSection, options?: { keepSidebar?: boolean }) => {
    if (section === "search") {
      if (viewRef.current.kind === "settings") navigate(previousView.current);
      else navigation.invalidateNavigation();
      setNavSection(section);
      setSidebarCollapsed(false);
      setSidebarOpen(true);
      return;
    }
    // 窄屏抽屉打开时，切换顶栏分区不收起抽屉：分区的列表就在抽屉里（技能、论坛、
    // 工作区、Agent 会话……），收起等于把刚切过去的内容一起藏起来。文件 ↔ Git 在桌面端
    // 也按标签处理（不改变侧栏开合），其它分区在桌面端仍按「进入分区」展开侧栏。
    const tabSwitch = (section === "files" || section === "git")
      && (viewRef.current.kind === "files" || viewRef.current.kind === "git");
    const keepSidebar = options?.keepSidebar || tabSwitch || (mobile && sidebarOpen);
    const navOptions = { keepSidebar };
    if (section === "agents" || section === "subagents") {
      const target = navigationMemory.restore(section, filteredAgents);
      const currentView = viewRef.current;
      if (target.kind === "chat" && target.sessionId !== null && !(currentView.kind === "chat"
        && currentView.agentName === target.agentName && currentView.sessionId === target.sessionId)) {
        void navigation.openSession(target.agentName, target.sessionId, navOptions);
      } else if (target.kind === "chat" && target.sessionId === null) {
        setActiveSession(null);
        navigate(target, true, navOptions);
      } else {
        // 回退总览可能只是当前工作区过滤了原 Agent，保留原有记忆。
        navigate(target, target.kind !== "home", navOptions);
      }
    } else if (section === "stats") {
      const target = selected && statsAgents.some(agent => agent.name === selected)
        ? selected
        : statsAgents[0]?.name ?? null;
      navigate({ kind: "stats", agentName: target }, true, navOptions);
    } else if (section === "files" || section === "git") {
      if (!previewWorkspacePaths[section] && currentWorkspaceTarget?.workspace) {
        setPreviewWorkspacePaths(previous => ({ ...previous, [section]: currentWorkspaceTarget.workspace }));
      }
      navigate({ kind: section, agentName: null }, true, navOptions);
    } else {
      navigate({ kind: section }, true, navOptions);
    }
    setNavSection(section);
    if (!keepSidebar) {
      setSidebarCollapsed(false);
      setSidebarOpen(false);
    }
  };
  const bindChatController = useCallback((controller: SessionController) => {
    const active = viewRef.current;
    if (active.kind === "chat" && active.agentName === controller.agentName) {
      navigationMemory.bindController(active.agentName, active.sessionId, controller);
    }
  }, [navigationMemory]);
  const workspaceDisplayName = activeWorkspace === "all" ? null : (workspaces.find(w => w.path === activeWorkspace)?.name || getWorkspaceBasename(activeWorkspace));
  const baseTitle = view.kind === "home" ? t(collection === "subagents" ? "nav.subagents" : "nav.workspace") : view.kind === "settings" ? t("nav.settings") : view.kind === "create" ? t("nav.newAgent")
    : view.kind === "skills" ? t("nav.skills") : view.kind === "stats" ? t("nav.stats") : view.kind === "forum" ? t("app.forum") : current?.name ?? "Pipi";
  const title = workspaceDisplayName && view.kind === "home" ? `${baseTitle} · ${workspaceDisplayName}` : baseTitle;
  const sessionTitle = view.kind === "chat" ? (sessionsByAgent[view.agentName] ?? []).find(session => session.id === view.sessionId)?.title : undefined;
  // 工作区已降级为概览页的分组维度：只有主 Agent 总览按组渲染，subagents 与单 Agent 视图保持平铺。
  const overviewGroups = view.kind === "home" && collection === "agents" ? workspaceGroups : null;
  const openWorkspaceSection = (section: "files" | "git", path: string) => {
    setPreviewWorkspacePaths(previous => ({ ...previous, [section]: path }));
    openSection(section);
  };
  const overviewProps = { section: collection, agents: visibleAgents, sessions: filteredSessionsByAgent, running: runningSessions, onAgent: selectAgent, onSession: openSession,
    groups: overviewGroups,
    groupActions: {
      onOpenFiles: (path: string) => openWorkspaceSection("files", path),
      onOpenGit: (path: string) => openWorkspaceSection("git", path),
      onRemoveCustom: handleRemoveCustomWorkspace,
    },
    onNewSession: (name: string) => { void navigation.startNewSession(name); }, onConfigure: configureAgent, onCreate: startCreating };

  if (authStatus.checking) return <div className="login-container" />;
  if (authStatus.authRequired && !authStatus.authenticated) return <Login onSuccess={() => {
    setAuthStatus(previous => ({ ...previous, authenticated: true })); safeSetError(null);
  }} />;

  return <LayoutContext.Provider value={layout}>
    <ForumProvider active={navSection === "forum" || view.kind === "forum"} agents={filteredAgents}>
      <div className={`app${sidebarOpen ? " sidebar-open" : ""}${sidebarCollapsed ? " sidebar-collapsed" : ""}`}
        style={{ "--sidebar-width": `${sidebarResize.width}px` } as React.CSSProperties}>
    <div className="app-body">
      <AppNavigation navSection={navSection} settingsOpen={view.kind === "settings"} onToggleSettings={toggleSettings}
        onHome={() => navigate({ kind: "home", section: "agents" })}
        activeWorkspace={activeWorkspace}
        running={{ agents: agentsInSection(filteredAgents, "agents").some(agent => agentHasRunning(agent.name)),
          subagents: agentsInSection(filteredAgents, "subagents").some(agent => agentHasRunning(agent.name)) }}
        onOpenSection={openSection} />
      <aside ref={sidebarRef} inert={mobile && !sidebarOpen} className="sidebar" id="primary-navigation" aria-label={view.kind === "settings" ? t("settings.title") : navSection === "stats" ? t("stats.title") : navSection === "files" || navSection === "git" ? t("app.sidebarFilesGit") : t("app.sidebarAgents")}>
        {/* 带局部过滤/搜索的分区不再叠一个全局快捷搜索按钮；⌘K 仍可用。 */}
        {view.kind === "settings" || navSection === "agents" || navSection === "subagents" ? (
          <button type="button" className="sidebar-quick" onClick={() => setPaletteOpen(true)}><IconSearch /><span>{t("nav.quickSearch")}</span><kbd>⌘ K</kbd></button>
        ) : null}
        {view.kind === "settings" ? settings && <SettingsSidebar settings={settings} section={settingsSection}
          showLogout={!isTauriRuntime() && authStatus.authRequired}
          onSelect={section => { setSettingsSection(section); setSidebarOpen(false); }} />
          : (navSection === "files" || navSection === "git") && (view.kind === "files" || view.kind === "git") ? <WorkspaceSidebar key={files.workspace.store?.key ?? "no-workspace"} state={files.workspace} workspaces={workspaces}
            onSelectWorkspace={wsPath => { setPreviewWorkspacePaths(previous => ({ ...previous, [view.kind === "git" ? "git" : "files"]: wsPath })); if (mobile) setSidebarOpen(true); }} />
          : navSection === "search" ? <SearchPanel query={searchQuery} onQueryChange={setSearchQuery} agents={filteredAgents}
          sessionsByAgent={filteredSessionsByAgent} isRunning={isConversationRunning} onSelectAgent={selectAgent} onOpenSession={openSession} />
          : navSection === "skills" ? <SkillsSidebar library={skillLibrary} selected={skillSelected} error={skillLoadError}
            onSelect={name => { setSkillSelected(name); setSidebarOpen(false); }} onRefresh={() => void loadSkillLibrary()} />
          : navSection === "forum" ? <ForumSidebar onSelect={() => { if (view.kind !== "forum") navigate({ kind: "forum" }); }} />
          : navSection === "stats" || view.kind === "stats" ? <StatsSidebar
            agents={statsAgents}
            selected={view.kind === "stats" ? view.agentName : selected}
            sessionsByAgent={filteredSessionsByAgent}
            onSelect={name => navigate({ kind: "stats", agentName: name })} />
          : <>
            {(navSection === "agents" || navSection === "subagents") && view.kind === "home" && (
              <WorkspaceFilterBar workspaces={workspaces} active={activeWorkspace} onSelect={handleSelectWorkspace} />
            )}
            <AgentSidebar key={collection} section={collection} agents={visibleAgents} archivedAgents={visibleArchivedAgents} selected={selected} chatOpen={view.kind === "chat"}
            viewSessionId={view.kind === "chat" ? view.sessionId : null}
            archivedSessions={archivedSessions} archivedSessionsExpanded={archivedSessionsExpanded} archivedOpen={archivedOpen}
            sessionsFor={sessionsFor} sessionStatus={sessionStatus} sessionUnread={sessionUnread}
            onSelectAgent={selectAgent} onConfigure={configureAgent}
            onStartNewSession={name => void navigation.startNewSession(name)} onOpenRepo={openRepo}
            onOpenSession={openSession}
            onArchiveAgent={name => void archive.archiveAgent(name)} onDeleteAgent={(name, archived) => void archive.deleteAgent(name, archived)}
            onRestoreAgent={name => void archive.restoreAgent(name)} onArchiveSession={(name, id) => void archive.archiveSession(name, id)}
            onDeleteSession={(name, id) => void archive.deleteSession(name, id)} onToggleArchivedSessions={name => void archive.toggleArchivedSessions(name)}
            onRestoreSession={(name, id) => void archive.restoreSession(name, id)} onDeleteArchivedSession={(name, id) => void archive.deleteArchivedSession(name, id)}
            onToggleArchivedOpen={() => setArchivedOpen(open => !open)} />
          </>}
      </aside>
      <div className="resize-handle sidebar-resize" title={t("app.resizeSidebar")} aria-label={t("app.resizeSidebarLabel")} {...sidebarResize.handleProps} />
      {/* 窄屏抽屉统一模态：点右侧内容（遮罩）收起，和右侧检查器一致。 */}
      {sidebarOpen && <button type="button" className="sidebar-backdrop" aria-label={t("app.closeNavigation")} onClick={closeNavigation} />}
      <main className="content-pane main" inert={mobile && sidebarOpen}>
        {(view.kind === "home" || view.kind === "agent" || (!current && (view.kind === "chat" || view.kind === "workbench"))) && <header className="workspace-toolbar">
          <NavigationToggle />
          <span className="toolbar-divider" /><span>{title}</span><span className="spacer" />
          <button type="button" className="icon-btn" aria-label={t("app.openQuickActions")} title={t("app.quickActionsShortcut")} onClick={() => setPaletteOpen(true)}><IconSearch /></button>
        </header>}
        {error && (
          <div className="error" role="alert">
            <IconAlertTriangle size={15} />
            <span>{error}</span>
            <button type="button" className="error-dismiss" aria-label={t("app.dismissError")} onClick={() => setError(null)}>
              <IconClose />
            </button>
          </div>
        )}
        {view.kind === "settings" && settings ? <SettingsView settings={settings} section={settingsSection} onChange={updateSettings} onClose={toggleSettings}
          showLogout={!isTauriRuntime() && authStatus.authRequired} />
          : view.kind === "create" ? <CreateAgentForm providers={settings?.providers ?? []} defaultProviderId={settings?.defaultProviderId ?? null}
            defaultWorkspace={activeWorkspace !== "all" ? activeWorkspace : ""}
            onCreated={async name => { await refresh(); selectAgent(name); }} onCancel={() => navigate({ kind: "home" })} onError={safeSetError} />
          : view.kind === "skills" ? <SkillsView library={skillLibrary} loadError={skillLoadError} selected={skillSelected}
            onSelect={setSkillSelected} onRefresh={loadSkillLibrary} onError={safeSetError} />
          : view.kind === "forum" ? null
          : (view.kind === "files" || view.kind === "git") ? <WorkspaceView key={files.workspace.store?.key ?? "no-workspace"} state={files.workspace}
            sessions={files.workspaceSessions} canReference={files.workspaceAgents.length > 0} defaultSession={files.workspaceSessions[0]?.id ?? null}
            onOpenFile={path => { files.openWorkspaceFile(files.currentWorkspaceTarget?.workspace ?? "", path); }} onReference={files.referenceToSession}
            onModeChange={mode => openSection(mode, { keepSidebar: true })} />
          : current && view.kind === "chat" && settings ? <ChatView key={`${current.name}-${chatKey}`} agent={current} providers={settings.providers}
            initialController={view.sessionId === null ? navigationMemory.controllerFor(current.name, null) : undefined} onControllerReady={bindChatController}
            onOpenFile={path => files.openWorkspaceFile(current.name, path)}
            title={sessionTitle || t("chat.newSessionTitle")} sessionId={view.sessionId} sessions={sessionRuntime}
            blockedSessionIds={[...navigation.blockedSessionIdsRef.current]} onBack={() => selectAgent(current.name)} onError={safeSetError}
            onRunningChange={handleRunningChange} onStopRequested={sessionId => markSessionInterrupted(current.name, sessionId)} onSessionReset={info => {
              setActiveSession(info ?? null);
              const active = viewRef.current;
              if (info && active.kind === "chat" && active.agentName === current.name) commitView({ ...active, sessionId: info.sessionId });
              void refreshSessions([current.name]);
              if (archivedSessionsExpanded[current.name]) void invoke("list_sessions", { agentName: current.name, archived: true })
                .then(list => setArchivedSessions(previous => ({ ...previous, [current.name]: list }))).catch(() => {});
            }} />
          : current && view.kind === "workbench" && settings ? <AgentWorkbench onOpenFile={path => files.openWorkspaceFile(current.name, path)} key={current.name} agent={current} providers={settings.providers}
            onSaved={async () => { await refresh(); }} onBack={() => selectAgent(current.name)} onError={safeSetError}
            blockedSessionIds={[...navigation.blockedSessionIdsRef.current]} sessions={sessionRuntime} onRunningChange={handleRunningChange} />
          : view.kind === "stats" ? <StatsView
            agent={statsAgents.find(item => item.name === view.agentName) ?? null}
            agents={statsAgents}
            onSelectAgent={name => navigate({ kind: "stats", agentName: name })}
            onOpenSession={openSession} />
          : <WorkspaceOverview {...overviewProps} agent={view.kind === "agent" ? current : undefined} />}
        {forumVisited && <div className="screen" inert={view.kind !== "forum"} style={{ display: view.kind === "forum" ? "flex" : "none" }}>
          <ForumView />
        </div>}
      </main>
    </div>
    {paletteOpen && <CommandPalette items={quickItems(filteredAgents, filteredSessionsByAgent, selected, t)} onChoose={chooseQuick} onClose={closePalette} />}
    {confirmState && <ConfirmModal title={confirmState.title} message={confirmState.message} onConfirm={confirmState.action} onCancel={() => setConfirmState(null)} />}
  </div></ForumProvider></LayoutContext.Provider>;
}
