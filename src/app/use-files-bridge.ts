import { useCallback, useMemo, useState } from "react";
import { useI18n } from "../i18n";
import type { AgentDefinition, SessionSummaryView } from "../types.ts";
import type { SessionRuntime } from "../session-runtime.ts";
import type { useSessionNavigation } from "./use-session-navigation.ts";
import type { AppView } from "./navigation.ts";
import {
  getWorkspaceBasename,
  normalizeWorkspacePath,
  resolveWorkspaceTarget,
  type WorkspaceInfo,
} from "./workspaces.ts";
import { useWorkspaceFiles } from "../files/use-workspace-files.ts";

/**
 * Files / Git 视图与全局状态之间的桥：目标工作区解析、工作区内 Agent 与会话、
 * 文件预览打开与「引用到会话」。纯桥接，不含导航状态机（切换分区仍在 App.tsx）。
 */
export function useFilesBridge(args: {
  agents: AgentDefinition[];
  filteredAgents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  activeWorkspace: string;
  workspaces: WorkspaceInfo[];
  current: AgentDefinition | null;
  viewKind: AppView["kind"];
  authenticated: boolean;
  agentRunning: boolean;
  navigate: (view: AppView, remember?: boolean, options?: { keepSidebar?: boolean }) => void;
  sessionRuntime: SessionRuntime;
  selectWorkspaceFilter: (path: string) => void;
  navigation: ReturnType<typeof useSessionNavigation>;
}) {
  const {
    agents, filteredAgents, sessionsByAgent, activeWorkspace, workspaces, current,
    viewKind, authenticated, agentRunning, navigate, sessionRuntime, navigation, selectWorkspaceFilter,
  } = args;
  const { t } = useI18n();
  const [previewWorkspacePaths, setPreviewWorkspacePaths] = useState<{ files: string | null; git: string | null }>({ files: null, git: null });

  const currentWorkspaceTarget = useMemo(() => {
    const mode = viewKind === "git" ? "git" : "files";
    const target = resolveWorkspaceTarget(previewWorkspacePaths[mode], activeWorkspace, current?.workspace, workspaces);
    return target ? { name: target.name, workspace: target.path } : null;
  }, [activeWorkspace, previewWorkspacePaths, current?.workspace, workspaces, viewKind]);

  const workspaceAgents = useMemo(() => {
    if (!currentWorkspaceTarget) return [];
    const targetNorm = normalizeWorkspacePath(currentWorkspaceTarget.workspace);
    return agents.filter(agent => !agent.subagent && normalizeWorkspacePath(agent.workspace) === targetNorm);
  }, [currentWorkspaceTarget, agents]);
  const workspaceSessions = useMemo(
    () => workspaceAgents.flatMap(agent => sessionsByAgent[agent.name] ?? []),
    [workspaceAgents, sessionsByAgent],
  );

  const workspaceActive = (viewKind === "files" || viewKind === "git") && authenticated;
  const workspace = useWorkspaceFiles(
    currentWorkspaceTarget,
    viewKind === "git" ? "git" : "files",
    workspaceActive,
    agentRunning,
  );

  const openWorkspaceFile = useCallback((agentOrWs: string, path: string) => {
    const agent = filteredAgents.find(item => item.name === agentOrWs) ?? agents.find(item => item.name === agentOrWs);
    const workspacePath = normalizeWorkspacePath(agent?.workspace || agentOrWs || currentWorkspaceTarget?.workspace);
    if (!workspacePath) return;
    const info = workspaces.find(item => normalizeWorkspacePath(item.path) === workspacePath);
    const target = { name: info?.name || getWorkspaceBasename(workspacePath), workspace: workspacePath };
    // 即使浏览器存储不可用，也复用目标工作区的内存视图。
    const store = workspace.getStore(target, "files");
    store.open(path);
    setPreviewWorkspacePaths(previous => ({ ...previous, files: workspacePath }));
    navigate({ kind: "files", agentName: agent?.name ?? null });
  }, [filteredAgents, agents, currentWorkspaceTarget?.workspace, workspaces, workspace, navigate]);

  const referenceToSession = useCallback((text: string, sessionId: string | null) => {
    const targetAgent = sessionId === null
      ? workspaceAgents.find(agent => agent.name === current?.name) ?? workspaceAgents[0]
      : workspaceAgents.find(agent => sessionsByAgent[agent.name]?.some(session => session.id === sessionId));
    if (!targetAgent) {
      throw new Error(t("app.noTargetAgent"));
    }
    const targetAgentName = targetAgent.name;
    if (activeWorkspace !== "all" && normalizeWorkspacePath(activeWorkspace) !== normalizeWorkspacePath(targetAgent.workspace)) {
      selectWorkspaceFilter("all");
    }
    const draft = sessionRuntime.composerDrafts.get(targetAgentName, sessionId);
    const previous = draft.getSnapshot().text;
    draft.write(previous ? `${previous}\n\n${text}` : text);
    if (sessionId === null) void navigation.startNewSession(targetAgentName);
    else void navigation.openSession(targetAgentName, sessionId);
  }, [workspaceAgents, current?.name, sessionsByAgent, sessionRuntime, navigation, activeWorkspace, selectWorkspaceFilter, t]);

  return {
    setPreviewWorkspacePaths,
    currentWorkspaceTarget,
    workspaceAgents,
    workspaceSessions,
    workspace,
    openWorkspaceFile,
    referenceToSession,
  };
}
