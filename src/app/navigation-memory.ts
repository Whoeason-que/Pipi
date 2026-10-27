import { agentSection, type AgentSection, type AppView } from "./navigation.ts";
import type { AgentDefinition } from "../types.ts";
import type { SessionController } from "../session-controller.ts";

type AgentView = Extract<AppView, { kind: "home" | "agent" | "workbench" | "chat" }>;
type Entry = { view: AgentView; controller?: SessionController };

/** 每个 Agent 分区记住最后的页面；临时表单与设置不覆盖它。 */
export class NavigationMemory {
  private entries = new Map<AgentSection, Entry>();

  remember(view: AppView, agents: AgentDefinition[]): void {
    if (view.kind !== "home" && view.kind !== "agent" && view.kind !== "workbench" && view.kind !== "chat") return;
    const section = view.kind === "home"
      ? view.section ?? "agents"
      : agentSection(agents.find(agent => agent.name === view.agentName));
    const previous = this.entries.get(section);
    const sameChat = view.kind === "chat" && previous?.view.kind === "chat"
      && previous.view.agentName === view.agentName && previous.view.sessionId === view.sessionId;
    this.entries.set(section, { view, controller: sameChat ? previous.controller : undefined });
  }

  restore(section: AgentSection, visibleAgents: AgentDefinition[]): AgentView {
    const view = this.entries.get(section)?.view;
    if (!view || ("agentName" in view && !visibleAgents.some(agent => agent.name === view.agentName && agentSection(agent) === section))) {
      return { kind: "home", section };
    }
    return view;
  }

  bindController(agentName: string, sessionId: string | null, controller: SessionController): void {
    for (const entry of this.entries.values()) {
      if (entry.view.kind === "chat" && entry.view.agentName === agentName && entry.view.sessionId === sessionId) {
        entry.controller = controller;
      }
    }
  }

  controllerFor(agentName: string, sessionId: string | null): SessionController | undefined {
    for (const entry of this.entries.values()) {
      if (entry.view.kind === "chat" && entry.view.agentName === agentName && entry.view.sessionId === sessionId) return entry.controller;
    }
    return undefined;
  }

  unbindDraft(agentName: string): void {
    for (const entry of this.entries.values()) {
      if (entry.view.kind === "chat" && entry.view.agentName === agentName && entry.view.sessionId === null) entry.controller = undefined;
    }
  }

  rekey(agentName: string, previous: string | null, next: string, controller: SessionController): boolean {
    let changed = false;
    for (const entry of this.entries.values()) {
      const view = entry.view;
      if (view.kind !== "chat" || view.agentName !== agentName || view.sessionId !== previous) continue;
      // 两个未落盘的新会话都可能是 null，只能更新拥有该控制器的那一个。
      if (previous === null && entry.controller !== controller) continue;
      entry.view = { ...view, sessionId: next };
      entry.controller = controller;
      changed = true;
    }
    return changed;
  }

  forgetSession(agentName: string, sessionId: string): void {
    for (const entry of this.entries.values()) {
      if (entry.view.kind === "chat" && entry.view.agentName === agentName && entry.view.sessionId === sessionId) {
        entry.view = { kind: "agent", agentName };
        entry.controller = undefined;
      }
    }
  }

  forgetAgent(agentName: string): void {
    for (const [section, entry] of this.entries) {
      if ("agentName" in entry.view && entry.view.agentName === agentName) this.entries.delete(section);
    }
  }
}
