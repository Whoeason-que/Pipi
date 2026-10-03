import { normalizeAgentEvent, normalizeStatsPayload } from "./chat-runtime.ts";
import type { Listen } from "./ipc.ts";
import { SessionController, type SessionFrame, type SessionTransport } from "./session-controller.ts";
import type { AgentDefinition, SessionChangedPayload } from "./types.ts";

export type RuntimeFrame = SessionFrame | { type: "session-changed"; payload: SessionChangedPayload };
export interface RuntimeTransport extends SessionTransport {
  listen: Listen;
}

const EVENT_NAMES = ["agent-event", "session-stats", "session-error", "approval-request", "session-switched", "session-changed"] as const;
const REPLAY_LIMIT = 2048;
const IDLE_CACHE_LIMIT = 32;

function key(agent: string, session: string): string { return `${agent}\u0000${session}`; }
function meta(frame: RuntimeFrame) {
  if (frame.type === "agent-event") return normalizeAgentEvent(frame.payload).meta;
  if (frame.type === "session-stats") return normalizeStatsPayload(frame.payload).meta;
  return frame.payload;
}

/** App 持有一份订阅与会话索引。视图只订阅控制器，不各自注册宿主事件。 */
export class SessionRuntime {
  private sessions = new Map<string, SessionController>();
  private drafts = new Set<SessionController>();
  private replay = new Map<string, { runId: number; frames: SessionFrame[] }>();
  private listeners = new Set<(frame: RuntimeFrame) => void>();
  private unlisten: Array<() => void> = [];
  private connection: Promise<void> | null = null;
  private connectionEpoch = 0;
  private transport: RuntimeTransport;

  constructor(transport: RuntimeTransport) { this.transport = transport; }

  forget(agent: string, session: string): void {
    this.sessions.delete(key(agent, session));
    this.replay.delete(key(agent, session));
  }

  forgetAgent(agent: string): void {
    const prefix = `${agent}\u0000`;
    for (const map of [this.sessions, this.replay]) {
      for (const sessionKey of map.keys()) if (sessionKey.startsWith(prefix)) map.delete(sessionKey);
    }
    for (const draft of this.drafts) if (draft.agentName === agent) this.drafts.delete(draft);
  }

  start(): Promise<void> {
    if (this.connection) return this.connection;
    const epoch = ++this.connectionEpoch;
    this.connection = Promise.allSettled(EVENT_NAMES.map(type => this.transport.listen(type,
      event => { if (epoch === this.connectionEpoch) this.receive({ type, payload: event.payload } as RuntimeFrame); },
    ))).then(results => {
      const subscriptions = results.flatMap(result => result.status === "fulfilled" ? [result.value] : []);
      const failed = results.find(result => result.status === "rejected");
      if (epoch !== this.connectionEpoch || failed) {
        subscriptions.forEach(unlisten => unlisten());
        if (epoch === this.connectionEpoch) this.connection = null;
        throw failed?.status === "rejected" ? failed.reason : new Error("事件订阅已结束");
      }
      this.unlisten = subscriptions;
    });
    return this.connection;
  }

  disconnect(): void {
    ++this.connectionEpoch;
    this.unlisten.forEach(unlisten => unlisten());
    this.unlisten = [];
    this.connection = null;
    // 断开的是宿主事件订阅，后台运行仍归 Rust RuntimeState 所有。
  }

  subscribe(listener: (frame: RuntimeFrame) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  get(agent: AgentDefinition, sessionId: string | null, temporary = false): SessionController {
    if (sessionId) {
      const existing = this.sessions.get(key(agent.name, sessionId));
      if (existing) return existing;
    }
    this.trim();
    const controller = new SessionController(agent, sessionId, temporary, this.transport,
      (current, previous, next) => {
        if (previous) this.sessions.delete(key(current.agentName, previous));
        this.drafts.delete(current);
        const nextKey = key(current.agentName, next);
        this.sessions.set(nextKey, current);
        const replay = this.replay.get(nextKey);
        this.replay.delete(nextKey);
        replay?.frames.forEach(frame => current.receive(frame));
      });
    if (sessionId) {
      const sessionKey = key(agent.name, sessionId);
      this.sessions.set(sessionKey, controller);
      const replay = this.replay.get(sessionKey);
      this.replay.delete(sessionKey);
      replay?.frames.forEach(frame => controller.receive(frame));
    } else this.drafts.add(controller);
    return controller;
  }

  receive(frame: RuntimeFrame): void {
    const identity = meta(frame);
    if (frame.type !== "session-changed") {
      if (identity) {
        const sessionKey = key(identity.agentName, identity.sessionId);
        let controller = this.sessions.get(sessionKey);
        if (!controller && frame.type === "agent-event" && normalizeAgentEvent(frame.payload).event.type === "agent_start") {
          controller = [...this.drafts].find(draft => draft.agentName === identity.agentName && draft.awaitingIdentity);
        }
        if (controller) controller.receive(frame);
        else this.remember(sessionKey, identity.runId, frame);
      } else {
        // 兼容旧的无 envelope 事件，仅交给正在观察的视图。
        for (const controller of new Set([...this.sessions.values(), ...this.drafts])) {
          if (controller.observed) controller.receive(frame);
        }
      }
    }
    this.listeners.forEach(listener => listener(frame));
  }

  private remember(sessionKey: string, runId: number, frame: SessionFrame): void {
    const current = this.replay.get(sessionKey);
    if (current && runId < current.runId) return;
    const frames = current?.runId === runId ? current.frames : [];
    if (frame.type === "agent-event" && normalizeAgentEvent(frame.payload).event.type === "agent_end") frames.length = 0;
    frames.push(frame);
    if (frames.length > REPLAY_LIMIT) frames.splice(0, frames.length - REPLAY_LIMIT);
    this.replay.delete(sessionKey);
    this.replay.set(sessionKey, { runId, frames });
    if (this.replay.size > IDLE_CACHE_LIMIT) this.replay.delete(this.replay.keys().next().value!);
  }

  private trim(): void {
    const idle = [...this.sessions.entries()].filter(([, controller]) => controller.evictable);
    for (const [sessionKey] of idle.slice(0, Math.max(0, idle.length - IDLE_CACHE_LIMIT))) this.sessions.delete(sessionKey);
    for (const draft of this.drafts) if (draft.evictable) this.drafts.delete(draft);
  }
}
