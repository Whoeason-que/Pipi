import type { AgentDefinition, SessionSummaryView } from "../types.ts";

export type AppView =
  | { kind: "home" | "create" | "skills" | "settings" }
  | { kind: "agent" | "workbench"; agentName: string }
  | { kind: "chat"; agentName: string; sessionId: string | null };

export type QuickTarget =
  | { kind: "agent"; agentName: string }
  | { kind: "chat"; agentName: string; sessionId: string }
  | { kind: "action"; action: "home" | "create" | "skills" | "settings" | "new-session" };
export interface QuickItem { id: string; label: string; detail: string; target: QuickTarget }

export function quickItems(agents: AgentDefinition[], sessions: Record<string, SessionSummaryView[]>, current: string | null): QuickItem[] {
  const actions: QuickItem[] = [
    { id: "home", label: "回到首页", detail: "浏览全部 Agent", target: { kind: "action", action: "home" } },
    { id: "create", label: "新建 Agent", detail: "创建一个持久的助手", target: { kind: "action", action: "create" } },
    { id: "skills", label: "打开技能库", detail: "管理能力包", target: { kind: "action", action: "skills" } },
    { id: "settings", label: "打开设置", detail: "模型供应商与外观", target: { kind: "action", action: "settings" } },
  ];
  if (current) actions.unshift({ id: "new-session", label: "新建会话", detail: current,
    target: { kind: "action", action: "new-session" } });
  const recent = agents.flatMap(agent => (sessions[agent.name] ?? []).map(session => ({ agent, session })))
    .sort((a, b) => b.session.lastActive - a.session.lastActive);
  return [...actions, ...agents.map(agent => ({ id: `agent:${agent.name}`, label: agent.name,
    detail: agent.description || "Agent 概览", target: { kind: "agent" as const, agentName: agent.name } })),
    ...recent.map(({ agent, session }) => ({ id: `session:${JSON.stringify([agent.name, session.id])}`,
      label: session.title || "未命名会话", detail: agent.name,
      target: { kind: "chat" as const, agentName: agent.name, sessionId: session.id } }))];
}

export function filterQuickItems(items: QuickItem[], query: string): QuickItem[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return items.filter(item => words.every(word => `${item.label} ${item.detail}`.toLocaleLowerCase().includes(word)))
    .slice(0, 60);
}
