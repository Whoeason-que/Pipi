import {
  INITIAL_CHAT_STATE, chatReducer, createEntryKey, eventMatchesRun, formatRuntimeError,
  normalizeAgentEvent, normalizeStatsPayload,
  type AgentEventPayload, type ApprovalDecisionValue, type ApprovalRequestPayload,
  type ChatAction, type ChatState,
  type SessionErrorPayload,
  type SessionEventMeta, type SessionStatsPayload, type SessionSwitchedPayload
} from "./chat-runtime.ts";
import type { Invoke } from "./ipc.ts";
import type { AgentDefinition, ModelConfig, SessionInfoView } from "./types.ts";

export interface SessionTransport {
  invoke: Invoke;
}

export type SessionFrame =
  | { type: "agent-event"; payload: AgentEventPayload }
  | { type: "session-stats"; payload: SessionStatsPayload }
  | { type: "session-error"; payload: SessionErrorPayload }
  | { type: "approval-request"; payload: ApprovalRequestPayload }
  | { type: "session-switched"; payload: SessionSwitchedPayload };

export interface SessionSnapshot {
  chat: ChatState;
  load: "idle" | "loading" | "ready" | "failed";
  operation: "idle" | "stopping";
  sessionId: string | null;
  model: ModelConfig | null;
  isCustomModel: boolean;
  approval: ApprovalRequestPayload | null;
  change: { revision: number; info: SessionInfoView } | null;
  error: { revision: number; message: string } | null;
}

/** 会话控制器只拥有前端投影。后端守卫与 JSONL 仍是运行和历史的权威。
 * 生命周期独立于视图；卸载订阅不会发送 stop_run。
 */
export class SessionController {
  private value: SessionSnapshot;
  private listeners = new Set<() => void>();
  private identity: { sessionId: string; runId?: number } | null;
  private settledRun: number | null = null;
  private awaitingRun: number | null = null;
  private ignoreEvents = false;
  private sending = false;
  private runEpoch = 0;
  private loadEpoch = 0;
  private pending: SessionFrame[] = [];
  private revision = 0;
  private agent: AgentDefinition;
  readonly temporary: boolean;
  private transport: SessionTransport;
  private rekey: (controller: SessionController, previous: string | null, next: string) => void;
  private delay: (ms: number) => Promise<void>;

  constructor(
    agent: AgentDefinition,
    sessionId: string | null,
    temporary: boolean,
    transport: SessionTransport,
    rekey: (controller: SessionController, previous: string | null, next: string) => void,
    delay: (ms: number) => Promise<void> = ms => new Promise(resolve => setTimeout(resolve, ms)),
  ) {
    this.agent = agent;
    this.temporary = temporary;
    this.transport = transport;
    this.rekey = rekey;
    this.delay = delay;
    this.identity = sessionId ? { sessionId } : null;
    this.value = {
      chat: INITIAL_CHAT_STATE, load: "idle", operation: "idle", sessionId,
      model: agent.provider ?? null, isCustomModel: false, approval: null, change: null, error: null,
    };
  }

  get agentName(): string { return this.agent.name; }
  get awaitingIdentity(): boolean { return this.value.chat.running && this.identity === null; }
  get observed(): boolean { return this.listeners.size > 0; }
  get evictable(): boolean {
    return !this.observed && !this.sending && !this.value.chat.running
      && this.value.operation === "idle" && this.value.load !== "loading";
  }
  getSnapshot = (): SessionSnapshot => this.value;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  updateAgent(agent: AgentDefinition): void {
    this.agent = agent;
    if (!this.value.isCustomModel && this.value.model !== agent.provider) this.patch({ model: agent.provider ?? null });
  }

  private patch(next: Partial<SessionSnapshot>): void {
    this.value = { ...this.value, ...next };
    this.listeners.forEach(listener => listener());
  }
  private dispatch(action: ChatAction): void { this.patch({ chat: chatReducer(this.value.chat, action) }); }
  private fail(error: unknown): void {
    this.patch({ error: { revision: ++this.revision, message: formatRuntimeError(error) } });
  }
  requireSessionId(): string {
    if (!this.identity) throw new Error("尚未确定会话身份，请稍后重试");
    return this.identity.sessionId;
  }

  private adopt(info: SessionInfoView, announce: boolean): void {
    const previous = this.value.sessionId;
    this.identity = { sessionId: info.sessionId, runId: info.runId };
    this.rekey(this, previous, info.sessionId);
    this.patch({
      sessionId: info.sessionId,
      ...(info.model !== undefined ? { model: info.model ?? this.agent.provider ?? null, isCustomModel: Boolean(info.isCustomModel) } : {}),
      ...(announce ? { change: { revision: ++this.revision, info } } : {}),
    });
  }

  private accept(meta: SessionEventMeta | null): boolean {
    if (!meta && (!this.identity || this.awaitingRun !== null || this.settledRun !== null)) return false;
    if (!eventMatchesRun(meta, this.agent.name, this.identity, this.settledRun, this.ignoreEvents)) return false;
    if (meta && this.awaitingRun !== null) {
      if (meta.runId <= this.awaitingRun) return false;
      this.awaitingRun = null;
      this.settledRun = null;
    }
    if (!meta) return true;
    if (!this.identity) {
      if (!this.awaitingIdentity) return false;
      this.adopt({ agentName: this.agent.name, sessionId: meta.sessionId,
        temporary: this.temporary, running: true, runId: meta.runId }, true);
    } else {
      if (meta.runId > (this.identity.runId ?? 0)) this.settledRun = null;
      this.identity = { sessionId: meta.sessionId, runId: Math.max(this.identity.runId ?? 0, meta.runId) };
    }
    return true;
  }

  receive(frame: SessionFrame): void {
    if (this.value.load === "idle" || this.value.load === "loading") {
      this.pending.push(frame);
      return;
    }
    if (this.value.load === "failed") return;
    this.apply(frame);
  }

  private apply(frame: SessionFrame): void {
    if (frame.type === "agent-event") {
      const { event, meta } = normalizeAgentEvent(frame.payload);
      const accepted = this.accept(meta);
      // 完成帧可补回漏掉的正文，但只能补当前已结算的 run。
      const recovery = !accepted && event.type === "agent_end" && Boolean(event.messages?.length)
        && meta !== null && this.awaitingRun === null && this.settledRun === meta.runId
        && this.identity?.sessionId === meta.sessionId && this.identity.runId === meta.runId;
      if (!accepted && !recovery) return;
      if (event.type === "agent_end") {
        this.settledRun = meta?.runId ?? this.identity?.runId ?? null;
        this.patch({ approval: null, operation: "idle" });
      }
      this.dispatch({ type: "event", event, key: createEntryKey("event") });
    } else if (frame.type === "session-stats") {
      const { stats, meta } = normalizeStatsPayload(frame.payload);
      if (this.accept(meta)) this.dispatch({ type: "stats", stats });
    } else if (frame.type === "session-switched") {
      const payload = frame.payload;
      if (!this.accept(payload)) return;
      this.adopt({ agentName: this.agent.name, sessionId: payload.toSessionId,
        runId: payload.runId, temporary: this.temporary, running: true }, true);
      // 只刷新会话信息；压缩后的时间线由 compaction_end 更新。
      void this.refreshInfo();
    } else if (frame.type === "approval-request") {
      if (this.accept(frame.payload)) this.patch({ approval: frame.payload });
    } else if (this.accept(frame.payload)) {
      this.fail(frame.payload.message);
    }
  }

  private async refreshInfo(): Promise<void> {
    const id = this.requireSessionId();
    const epoch = this.runEpoch;
    try {
      const info = await this.transport.invoke("session_info", { agentName: this.agent.name, sessionId: id });
      if (info && this.identity?.sessionId === id && this.runEpoch === epoch
        && (info.runId ?? 0) >= (this.identity.runId ?? 0)) this.adopt(info, true);
    } catch (error) { if (this.identity?.sessionId === id && this.runEpoch === epoch) this.fail(error); }
  }

  /** 先订阅，再取快照；加载期间的所有事件按到达顺序合并。 */
  async hydrate(): Promise<void> {
    const epoch = ++this.loadEpoch;
    const id = this.value.sessionId;
    this.patch({ load: "loading", error: null });
    if (!id) {
      this.dispatch({ type: "hydrate", messages: [], stats: null, running: false });
      this.patch({ load: "ready" });
    } else {
      const args = { agentName: this.agent.name, sessionId: id };
      const [messages, stats, running, info] = await Promise.allSettled([
        this.transport.invoke("session_messages", args),
        this.transport.invoke("session_stats", args),
        this.transport.invoke("session_running", args),
        this.transport.invoke("session_info", args),
      ]);
      if (epoch !== this.loadEpoch) return;
      const identityValid = info.status === "fulfilled" && info.value !== null
        && info.value.agentName === this.agent.name && info.value.sessionId === id;
      if (!identityValid || messages.status === "rejected" || stats.status === "rejected" || running.status === "rejected") {
        this.pending = [];
        this.patch({ load: "failed" });
        const rejected = [messages, stats, running, info].find(result => result.status === "rejected");
        this.fail(rejected?.status === "rejected" ? rejected.reason : "无法确认当前会话身份，请返回后重试");
        return;
      }
      if (info.status === "fulfilled" && info.value) {
        const knownRun = this.identity?.runId ?? 0;
        const stale = (info.value.runId ?? 0) < knownRun;
        if (!stale) {
          this.adopt(info.value, false);
          this.settledRun = info.value.running ? null : (info.value.runId ?? null);
        }
      }
      // 新快照是 JSONL 的权威；现有 transient 消息由 reducer 合并。
      const previous = this.value.chat;
      this.patch({ chat: { ...previous, stats: null, liveEventSeen: false } });
      this.dispatch({ type: "hydrate", messages: messages.value, stats: stats.value, running: (info.value?.runId ?? 0) < (this.identity?.runId ?? 0) ? previous.running : running.value });
      this.patch({ load: "ready" });
    }
    const pending = this.pending;
    this.pending = [];
    pending.forEach(frame => this.apply(frame));
  }

  async send(text: string): Promise<boolean> {
    text = text.trim();
    if (!text || this.value.load !== "ready" || this.sending) return false;
    this.sending = true;
    if (this.value.chat.running) {
      try { await this.transport.invoke("steer", { agentName: this.agent.name, sessionId: this.requireSessionId(), message: text }); }
      finally { this.sending = false; }
      return true;
    }
    const key = createEntryKey("user");
    ++this.runEpoch;
    this.awaitingRun = this.identity?.runId ?? null;
    this.settledRun = null;
    this.ignoreEvents = false;
    this.dispatch({ type: "submit_user", key, text });
    try {
      await this.transport.invoke("send_prompt", { agentName: this.agent.name,
        sessionId: this.value.sessionId, prompt: text, model: this.value.isCustomModel ? this.value.model : null });
      return true;
    } catch (error) {
      this.dispatch({ type: "reject_user", key });
      this.awaitingRun = null;
      throw error;
    } finally { this.sending = false; }
  }

  async stop(): Promise<void> {
    if (!this.value.chat.running || this.value.operation === "stopping") return;
    const epoch = this.runEpoch;
    this.patch({ operation: "stopping" });
    try {
      await this.transport.invoke("stop_run", { agentName: this.agent.name, sessionId: this.requireSessionId() });
      for (let attempt = 0; attempt <= 50; attempt++) {
        if (epoch !== this.runEpoch || this.getSnapshot().operation !== "stopping") return;
        const running = await this.transport.invoke("session_running", {
          agentName: this.agent.name, sessionId: this.requireSessionId(),
        });
        if (epoch !== this.runEpoch || this.getSnapshot().operation !== "stopping") return;
        if (!running) {
          this.ignoreEvents = true;
          await this.hydrate();
          if (epoch !== this.runEpoch) return;
          this.settledRun = this.identity?.runId ?? this.settledRun;
          this.dispatch({ type: "event", event: { type: "agent_end" }, key: createEntryKey("stop") });
          this.patch({ operation: "idle", approval: null });
          return;
        }
        if (attempt === 50) throw new Error("停止请求已发送，但后端仍在运行；请稍后重试");
        await this.delay(100);
      }
    } catch (error) {
      if (epoch === this.runEpoch) { this.patch({ operation: "idle" }); this.fail(error); }
    }
  }

  async resolveApproval(decision: ApprovalDecisionValue): Promise<void> {
    const request = this.value.approval;
    if (!request) return;
    this.patch({ approval: null });
    await this.transport.invoke("resolve_approval", { requestId: request.requestId, decision });
  }
  async compact(): Promise<void> {
    await this.transport.invoke("compact_now", { agentName: this.agent.name, sessionId: this.requireSessionId() });
  }
  async fork(): Promise<void> {
    if (this.value.load !== "ready" || this.value.chat.running) return;
    const info = await this.transport.invoke("fork_session", {
      agentName: this.agent.name, sessionId: this.requireSessionId(), upToEntryId: null,
    });
    this.adopt(info, true);
    this.settledRun = null;
    this.patch({ chat: INITIAL_CHAT_STATE });
    await this.hydrate();
  }
  async setModel(isCustom: boolean, model: ModelConfig | null): Promise<void> {
    if (this.identity) await this.transport.invoke("set_session_model", {
      agentName: this.agent.name, sessionId: this.identity.sessionId, model: isCustom ? model : null,
    });
    this.patch({ model: isCustom ? model : this.agent.provider ?? null, isCustomModel: isCustom });
  }
}
