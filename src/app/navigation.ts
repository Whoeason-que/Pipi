import { getTranslation } from "../i18n/core.ts";
import type { AgentDefinition, SessionSummaryView } from "../types.ts";

export type AgentSection = "agents" | "subagents";
export type NavSection = AgentSection | "search" | "skills" | "forum" | "files" | "git" | "stats";

export function agentSection(agent: Pick<AgentDefinition, "subagent"> | undefined): AgentSection {
  return agent?.subagent ? "subagents" : "agents";
}

export function agentsInSection(agents: AgentDefinition[], section: AgentSection): AgentDefinition[] {
  return agents.filter(agent => agentSection(agent) === section);
}

export type AppView =
  | { kind: "home"; section?: AgentSection }
  | { kind: "create" | "skills" | "settings" | "forum" }
  | { kind: "agent" | "workbench"; agentName: string }
  | { kind: "files" | "git"; agentName: string | null }
  | { kind: "stats"; agentName: string | null }
  | { kind: "chat"; agentName: string; sessionId: string | null };

export type QuickTarget =
  | { kind: "agent"; agentName: string }
  | { kind: "chat"; agentName: string; sessionId: string }
  | { kind: "action"; action: "home" | "create" | "skills" | "settings" | "new-session" | "files" | "git" | "stats" };
export interface QuickItem { id: string; label: string; detail: string; target: QuickTarget }

export type Translate = (path: string, params?: Record<string, string | number>) => string;

/** 默认英文（权威字典）；调用方传入 useI18n 的 t 以跟随当前语言。 */
const ENGLISH: Translate = (path, params) => getTranslation("en", path, params);

export function quickItems(agents: AgentDefinition[], sessions: Record<string, SessionSummaryView[]>, current: string | null, t: Translate = ENGLISH): QuickItem[] {
  const actions: QuickItem[] = [
    { id: "home", label: t("app.quickHome"), detail: t("app.quickHomeDetail"), target: { kind: "action", action: "home" } },
    { id: "stats", label: t("stats.title"), detail: t("app.quickStatsDetail"), target: { kind: "action", action: "stats" } },
    { id: "create", label: t("nav.newAgent"), detail: t("app.quickCreateAgentDetail"), target: { kind: "action", action: "create" } },
    { id: "files", label: t("app.quickFiles"), detail: current || t("app.quickWorkspaceFallback"), target: { kind: "action", action: "files" } },
    { id: "git", label: t("app.quickGit"), detail: current || t("app.quickWorkspaceFallback"), target: { kind: "action", action: "git" } },
    { id: "skills", label: t("app.quickSkills"), detail: t("app.quickSkillsDetail"), target: { kind: "action", action: "skills" } },
    { id: "settings", label: t("nav.openSettings"), detail: t("app.quickSettingsDetail"), target: { kind: "action", action: "settings" } },
  ];
  if (current) actions.unshift({ id: "new-session", label: t("nav.newSession"), detail: current,
    target: { kind: "action", action: "new-session" } });
  const recent = agents.flatMap(agent => (sessions[agent.name] ?? []).map(session => ({ agent, session })))
    .sort((a, b) => b.session.lastActive - a.session.lastActive);
  return [...actions, ...agents.map(agent => ({ id: `agent:${agent.name}`, label: agent.name,
    detail: agent.description || t("app.agentOverview"), target: { kind: "agent" as const, agentName: agent.name } })),
    ...recent.map(({ agent, session }) => ({ id: `session:${JSON.stringify([agent.name, session.id])}`,
      label: session.title || t("workspace.untitled"), detail: agent.name,
      target: { kind: "chat" as const, agentName: agent.name, sessionId: session.id } }))];
}

export function filterQuickItems(items: QuickItem[], query: string): QuickItem[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return items.filter(item => words.every(word => `${item.label} ${item.detail}`.toLocaleLowerCase().includes(word)))
    .slice(0, 60);
}
