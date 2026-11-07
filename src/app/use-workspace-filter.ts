import { useCallback, useMemo, useState } from "react";
import type { AgentDefinition, SessionSummaryView } from "../types.ts";
import {
  buildWorkspaceGroups,
  buildWorkspaceList,
  loadActiveWorkspace,
  loadCustomWorkspaces,
  normalizeWorkspacePath,
  saveActiveWorkspace,
  saveCustomWorkspaces,
  type WorkspaceGroupInfo,
  type WorkspaceInfo,
} from "./workspaces.ts";

/**
 * 全局工作区过滤器（localStorage 持久化）与派生数据。
 * 工作区只是 Agent 的组织维度：过滤器、概览页分组与 Files/Git 侧栏共用这一份。
 * 切换过滤器引发的兜底导航留在 App.tsx —— 那半段依赖视图状态机。
 */
export function useWorkspaceFilter(args: {
  agents: AgentDefinition[];
  archivedAgents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
}) {
  const { agents, archivedAgents, sessionsByAgent } = args;
  // 自定义工作区只减不增：历史 localStorage 条目保留为概览页的空组，新增入口已随
  // WorkspacesView 一起移除，需要时在创建 Agent 时直接填目录。
  const [customWorkspaces, setCustomWorkspaces] = useState(() => loadCustomWorkspaces());
  const [activeWorkspace, setActiveWorkspace] = useState(() => loadActiveWorkspace());

  const workspaces: WorkspaceInfo[] = useMemo(
    () => buildWorkspaceList(agents, customWorkspaces),
    [agents, customWorkspaces],
  );

  const filteredAgents = useMemo(() => {
    if (activeWorkspace === "all") return agents;
    const target = normalizeWorkspacePath(activeWorkspace);
    return agents.filter((agent) => agent.subagent || normalizeWorkspacePath(agent.workspace) === target);
  }, [agents, activeWorkspace]);

  const filteredArchivedAgents = useMemo(() => {
    if (activeWorkspace === "all") return archivedAgents;
    const target = normalizeWorkspacePath(activeWorkspace);
    return archivedAgents.filter((agent) => agent.subagent || normalizeWorkspacePath(agent.workspace) === target);
  }, [archivedAgents, activeWorkspace]);

  const filteredSessionsByAgent = useMemo(() => {
    if (activeWorkspace === "all") return sessionsByAgent;
    const result: Record<string, SessionSummaryView[]> = {};
    for (const agent of filteredAgents) {
      if (sessionsByAgent[agent.name]) {
        result[agent.name] = sessionsByAgent[agent.name];
      }
    }
    return result;
  }, [sessionsByAgent, filteredAgents, activeWorkspace]);

  // 概览页分组与可见 Agent 同源：跟随当前过滤器，过滤到单个工作区时只剩一组。
  const workspaceGroups: WorkspaceGroupInfo[] = useMemo(
    () => buildWorkspaceGroups(filteredAgents, customWorkspaces),
    [filteredAgents, customWorkspaces],
  );

  const select = useCallback((next: string) => {
    setActiveWorkspace(next);
    saveActiveWorkspace(next);
  }, []);

  const removeCustom = useCallback((path: string) => {
    const norm = normalizeWorkspacePath(path);
    setCustomWorkspaces((previous) => {
      const next = previous.filter((item) => normalizeWorkspacePath(item.path) !== norm);
      saveCustomWorkspaces(next);
      return next;
    });
  }, []);

  return {
    activeWorkspace,
    workspaces,
    workspaceGroups,
    filteredAgents,
    filteredArchivedAgents,
    filteredSessionsByAgent,
    select,
    removeCustom,
  };
}
