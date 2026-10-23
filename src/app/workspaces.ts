// 工作区（Workspace）数据结构与本地持久化助手。
// 一个 Workspace 代表一个本地目录路径；支持全部（all）与单个 Workspace 切换。

import type { AgentDefinition } from "../types";

export interface WorkspaceInfo {
  path: string; // 归一化路径，若是全部则为 "all"
  name: string; // 展示名称（文件夹名或自定义名）
  agentCount: number;
  custom?: boolean; // 是否为手动添加的工作区
}

export interface CustomWorkspace {
  path: string;
  name?: string;
}

export function normalizeWorkspacePath(path: string | null | undefined): string {
  if (!path) return "";
  const trimmed = path.trim();
  if (!trimmed) return "";
  // 移除尾部斜杠（保留单个根目录 /）
  return trimmed.length > 1 && (trimmed.endsWith("/") || trimmed.endsWith("\\"))
    ? trimmed.slice(0, -1)
    : trimmed;
}

export function getWorkspaceBasename(path: string): string {
  if (!path) return "";
  const normalized = normalizeWorkspacePath(path);
  const parts = normalized.split(/[/\\]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : normalized;
}

const CUSTOM_WORKSPACES_KEY = "pipi:custom-workspaces";
const ACTIVE_WORKSPACE_KEY = "pipi:active-workspace";

export function loadCustomWorkspaces(): CustomWorkspace[] {
  try {
    const raw = localStorage.getItem(CUSTOM_WORKSPACES_KEY);
    if (!raw) return [];
    const list = JSON.parse(raw);
    if (Array.isArray(list)) {
      return list.filter(
        (item) => item && typeof item.path === "string" && item.path.trim(),
      );
    }
  } catch {
    // 忽略异常
  }
  return [];
}

export function saveCustomWorkspaces(list: CustomWorkspace[]) {
  try {
    localStorage.setItem(CUSTOM_WORKSPACES_KEY, JSON.stringify(list));
  } catch {
    // 忽略异常
  }
}

export function loadActiveWorkspace(): string {
  try {
    return localStorage.getItem(ACTIVE_WORKSPACE_KEY) || "all";
  } catch {
    return "all";
  }
}

export function saveActiveWorkspace(ws: string) {
  try {
    localStorage.setItem(ACTIVE_WORKSPACE_KEY, ws);
  } catch {
    // 忽略异常
  }
}

/**
 * 汇总计算当前所有可用的 Workspace 列表（合并 Agent 的 workspace 与自定义添加的 workspace）。
 */
export function buildWorkspaceList(
  agents: AgentDefinition[],
  customWorkspaces: CustomWorkspace[],
): WorkspaceInfo[] {
  const map = new Map<string, WorkspaceInfo>();

  // 1. 先加入自定义添加的工作区（初始 agentCount 设为 0）
  for (const custom of customWorkspaces) {
    const norm = normalizeWorkspacePath(custom.path);
    if (!norm) continue;
    map.set(norm, {
      path: norm,
      name: custom.name?.trim() || getWorkspaceBasename(norm),
      agentCount: 0,
      custom: true,
    });
  }

  // 2. 统计所有 Agent 的 workspace（工作区只关注主 Agent，不看 Subagent）
  for (const agent of agents) {
    if (agent.subagent) continue;
    const norm = normalizeWorkspacePath(agent.workspace);
    if (!norm) continue;
    const existing = map.get(norm);
    if (existing) {
      existing.agentCount += 1;
    } else {
      map.set(norm, {
        path: norm,
        name: getWorkspaceBasename(norm),
        agentCount: 1,
        custom: false,
      });
    }
  }

  return Array.from(map.values()).sort((a, b) => a.name.localeCompare(b.name));
}
