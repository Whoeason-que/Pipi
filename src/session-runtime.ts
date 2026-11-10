import type { Listen } from "./ipc.ts";
import { DraftStore } from "./drafts.ts";
import { SessionController, type SessionFrame, type SessionTransport } from "./session-controller.ts";
import { sessionKey as composeSessionKey } from "./session-key.ts";
import type { AgentDefinition, SessionChangedPayload } from "./types.ts";
import { forgetAgentBrowsers, forgetBrowserSession, rekeyBrowserSession } from "./chat/browser-sessions.ts";

export type RuntimeFrame = SessionFrame | { type: "session-changed"; payload: SessionChangedPayload };
export interface RuntimeTransport extends SessionTransport {
  listen: Listen;
}

const EVENT_NAMES = ["agent-event", "session-stats", "session-error", "session-switched", "session-changed", "prompt-queue"] as const;
const REPLAY_LIMIT = 2048;
const IDLE_CACHE_LIMIT = 32;

/** App 持有一份订阅与会话索引。视图只订阅控制器，不各自注册宿主事件。 */
export class SessionRuntime {
  readonly composerDrafts = new DraftStore();
  private sessions = new Map<string, SessionController>();
  private drafts = new Set<SessionController>();
  private replay = new Map<string, { runId: number; frames: SessionFrame[] }>();
  /** 未注册 controller 的子会话也能从事件流判断是否还在跑（agent_end 清除）。 */
  private live = new Set<string>();
  private listeners = new Set<(frame: RuntimeFrame) => void>();
  private rekeyListeners = new Set<(controller: SessionController, previous: string | null, next: string, temporary: boolean) => void>();
  private unlisten: Array<() => void> = [];
  private connection: Promise<void> | null = null;
  private connectionEpoch = 0;
  private transport: RuntimeTransport;

  constructor(transport: RuntimeTransport) { this.transport = transport; }

  forget(agent: string, session: string): void {
    forgetBrowserSession(agent, session);
    this.composerDrafts.forget(agent, session);
    this.sessions.delete(composeSessionKey(agent, session));
    this.replay.delete(composeSessionKey(agent, session));
    this.live.delete(composeSessionKey(agent, session));
  }

  forgetAgent(agent: string): void {
    forgetAgentBrowsers(agent);
    this.composerDrafts.forgetAgent(agent);
    const prefix = `${agent}\u0000`;
    for (const map of [this.sessions, this.replay]) {
      for (const sessionKey of map.keys()) if (sessionKey.startsWith(prefix)) map.delete(sessionKey);
    }
    for (const sessionKey of this.live) if (sessionKey.startsWith(prefix)) this.live.delete(sessionKey);
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
        throw failed?.status === "rejected" ? failed.reason : new Error("Event subscription ended");
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
    // 断开的是宿主事件订阅，后台运行仍归 Rust RuntimeState 所有；
    // 连接期间收到的运行态标记可能已经过期，重连后按新事件重建。
    this.live.clear();
  }

  async resynchronize(): Promise<void> {
    // 重连后不能信任断线期间的旧帧；从宿主恢复正在展示或运行中的会话。
    this.replay.clear();
    const controllers = [...new Set([...this.sessions.values(), ...this.drafts])]
      .filter(controller => controller.getSnapshot().sessionId !== null && (controller.observed || controller.getSnapshot().chat.running));
    await Promise.allSettled(controllers.map(controller => controller.reconnect()));
  }

  subscribe(listener: (frame: RuntimeFrame) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /** 没有注册 controller 的会话（例如 inline 子运行）也能判断是否还在跑。 */
  isRunning(agent: string, session: string): boolean {
    return this.live.has(composeSessionKey(agent, session));
  }

  subscribeRekeys(listener: (controller: SessionController, previous: string | null, next: string, temporary: boolean) => void): () => void {
    this.rekeyListeners.add(listener);
    return () => { this.rekeyListeners.delete(listener); };
  }

  /** 返回未落盘的会话时重新登记控制器，允许它接收先于创建响应到达的事件。 */
  retainDraft(controller: SessionController): void {
    if (controller.getSnapshot().sessionId === null) this.drafts.add(controller);
  }

  get(agent: AgentDefinition, sessionId: string | null, temporary = false): SessionController {
    if (sessionId) {
      const existing = this.sessions.get(composeSessionKey(agent.name, sessionId));
      if (existing) return existing;
    }
    this.trim();
    const controller = new SessionController(agent, sessionId, temporary, this.transport,
      (current, previous, next) => {
        if (previous && previous !== next) rekeyBrowserSession(current.agentName, previous, next);
        this.composerDrafts.move(current.agentName, previous, next, temporary);
        if (previous) this.sessions.delete(composeSessionKey(current.agentName, previous));
        this.drafts.delete(current);
        const nextKey = composeSessionKey(current.agentName, next);
        this.sessions.set(nextKey, current);
        if (previous !== next) this.rekeyListeners.forEach(listener => listener(current, previous, next, temporary));
        const replay = this.replay.get(nextKey);
        this.replay.delete(nextKey);
        replay?.frames.forEach(frame => current.receive(frame));
      });
    if (sessionId) {
      const sessionKey = composeSessionKey(agent.name, sessionId);
      this.sessions.set(sessionKey, controller);
      const replay = this.replay.get(sessionKey);
      this.replay.delete(sessionKey);
      replay?.frames.forEach(frame => controller.receive(frame));
    } else this.drafts.add(controller);
    return controller;
  }

  receive(frame: RuntimeFrame): void {
    // 每个载荷都自带身份（见 chat-runtime 的 AgentEventPayload / *Payload）。
    const identity = frame.payload;
    if (frame.type !== "session-changed") {
      const sessionKey = composeSessionKey(identity.agentName, identity.sessionId);
      let controller = this.sessions.get(sessionKey);
      if (!controller && frame.type === "agent-event" && frame.payload.event.type === "agent_start") {
        // 事件先于 send_prompt 响应到达时的兜底：主路径是响应里的权威身份
        // （见 session-controller.send），这里按 Agent 名接管第一条 agent_start。
        controller = [...this.drafts].find(draft => draft.agentName === identity.agentName && draft.awaitingIdentity);
      }
      if (controller) controller.receive(frame);
      else this.remember(sessionKey, identity.runId, frame);
    }
    if (frame.type === "agent-event") {
      const key = composeSessionKey(frame.payload.agentName, frame.payload.sessionId);
      if (frame.payload.event.type === "agent_end") this.live.delete(key);
      else this.live.add(key);
    } else if (frame.type === "session-switched") {
      const from = composeSessionKey(frame.payload.agentName, frame.payload.sessionId);
      if (this.live.delete(from)) {
        this.live.add(composeSessionKey(frame.payload.agentName, frame.payload.toSessionId));
      }
    }
    this.listeners.forEach(listener => listener(frame));
  }

  private remember(sessionKey: string, runId: number, frame: SessionFrame): void {
    const current = this.replay.get(sessionKey);
    if (current && runId < current.runId) return;
    const frames = current?.runId === runId ? current.frames : [];
    if (frame.type === "agent-event" && frame.payload.event.type === "agent_end") frames.length = 0;
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
