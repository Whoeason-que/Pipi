import {
  INITIAL_CHAT_STATE, chatReducer, createEntryKey, eventMatchesRun, formatRuntimeError,
  type AgentEventPayload, type ApprovalDecisionValue, type ApprovalRequestPayload,
  type ChatAction, type ChatState,
  type SessionErrorPayload,
  type SessionEventMeta, type SessionStatsPayload, type SessionSwitchedPayload
} from "./chat-runtime.ts";
import { CommandTransportError } from "./transport-error.ts";
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
  operation: "idle" | "stopping" | "uncertain";
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
  private sending = false;
  private uncertainSend: { runId: number; key: string } | null = null;
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

  private accept(meta: SessionEventMeta): boolean {
    if (!eventMatchesRun(meta, this.agent.name, this.identity, this.settledRun)) return false;
    if (this.awaitingRun !== null) {
      if (meta.runId <= this.awaitingRun) return false;
      this.awaitingRun = null;
      this.settledRun = null;
    }
    if (!this.identity) {
      if (!this.awaitingIdentity) return false;
      this.adopt({ agentName: this.agent.name, sessionId: meta.sessionId,
        temporary: this.temporary, running: true, runId: meta.runId }, true);
    } else {
      if (meta.runId > (this.identity.runId ?? 0)) this.settledRun = null;
      this.identity = { sessionId: meta.sessionId, runId: Math.max(this.identity.runId ?? 0, meta.runId) };
    }
    if (this.uncertainSend && meta.runId > this.uncertainSend.runId) {
      this.uncertainSend = null;
      this.patch({ error: null, operation: this.value.operation === "stopping" ? "stopping" : "idle" });
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
      const meta = frame.payload;
      const event = meta.event;
      const accepted = this.accept(meta);
      // 完成帧可补回漏掉的正文，但只能补当前已结算的 run。
      const recovery = !accepted && event.type === "agent_end" && Boolean(event.messages?.length)
        && this.awaitingRun === null && this.settledRun === meta.runId
        && this.identity?.sessionId === meta.sessionId && this.identity.runId === meta.runId;
      if (!accepted && !recovery) return;
      if (event.type === "agent_end") {
        this.settledRun = meta?.runId ?? this.identity?.runId ?? null;
        this.patch({ approval: null, operation: "idle" });
      }
      this.dispatch({ type: "event", event, key: createEntryKey("event") });
    } else if (frame.type === "session-stats") {
      const meta = frame.payload;
      if (this.accept(meta)) this.dispatch({ type: "stats", stats: meta.stats });
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
  async reconnect(): Promise<void> {
    this.patch({ approval: null });
    await this.hydrate();
  }

  async hydrate(): Promise<void> {
    const epoch = ++this.loadEpoch;
    const id = this.value.sessionId;
    this.patch({ load: "loading", error: null });
    if (!id) {
      this.dispatch({ type: "hydrate", messages: [], stats: null, running: false });
      this.patch({ load: "ready" });
    } else {
      const args = { agentName: this.agent.name, sessionId: id };
      try {
        const snapshot = await this.transport.invoke("open_session", args);
        if (epoch !== this.loadEpoch) return;
        const info = snapshot.info;
        const identityValid = info !== null && info.agentName === this.agent.name && info.sessionId === id;
        if (!identityValid) {
          this.pending = [];
          this.patch({ load: "failed" });
          this.fail("无法确认当前会话身份，请返回后重试");
          return;
        }
        const knownRun = this.identity?.runId ?? 0;
        const stale = (info.runId ?? 0) < knownRun;
        if (!stale) {
          this.adopt(info, false);
          this.settledRun = info.running ? null : (info.runId ?? null);
        }
        // 新快照是 JSONL 的权威；现有 transient 消息由 reducer 合并。
        const previous = this.value.chat;
        this.patch({ chat: { ...previous, stats: null, liveEventSeen: false } });
        this.dispatch({
          type: "hydrate",
          messages: snapshot.messages,
          stats: snapshot.stats,
          running: this.uncertainSend ? true : stale ? previous.running : info.running,
        });
        const unresolved = this.uncertainSend;
        const confirmedPrompt = unresolved && this.value.chat.entries.some(entry =>
          entry.key === unresolved.key && !entry.transient);
        if (unresolved && (confirmedPrompt || (info.running && (info.runId ?? 0) > unresolved.runId))) {
          this.uncertainSend = null;
          if (this.value.operation === "uncertain") this.patch({ operation: "idle" });
          this.dispatch({ type: "running", running: stale ? previous.running : info.running });
        }
        this.patch({ load: "ready" });
      } catch (error) {
        if (epoch !== this.loadEpoch) return;
        this.pending = [];
        this.patch({ load: "failed" });
        this.fail(error instanceof Error ? error.message : String(error));
        return;
      }
    }
    const pending = this.pending;
    this.pending = [];
    pending.forEach(frame => this.apply(frame));
  }

  async send(text: string): Promise<boolean> {
    text = text.trim();
    if (!text || this.value.load !== "ready" || this.sending || this.uncertainSend) return false;
    this.sending = true;
    if (this.value.chat.running) {
      try { await this.transport.invoke("steer", { agentName: this.agent.name, sessionId: this.requireSessionId(), message: text }); }
      finally { this.sending = false; }
      return true;
    }
    // 先取得稳定身份：响应丢失后仍能查到同一条会话，且不会误认其他窗口的运行。
    if (!this.identity) {
      try {
        const info = this.temporary
          ? await this.transport.invoke("open_workbench_session", { agentName: this.agent.name })
          : await this.transport.invoke("create_session", { agentName: this.agent.name });
        if (info.agentName !== this.agent.name || !info.sessionId || info.running) {
          throw new Error("Unable to establish an idle session");
        }
        // 创建身份不应覆盖草稿中已选择的模型。
        this.adopt({ ...info, model: undefined }, true);
      } catch (error) { this.sending = false; throw error; }
    }
    const baseline = this.identity?.runId ?? 0;
    const requestedSession = this.requireSessionId();
    const key = createEntryKey("user");
    ++this.runEpoch;
    this.awaitingRun = baseline;
    this.settledRun = null;
    this.dispatch({ type: "submit_user", key, text });
    try {
      const info = await this.transport.invoke("send_prompt", { agentName: this.agent.name,
        sessionId: requestedSession, prompt: text, model: this.value.isCustomModel ? this.value.model : null });
      if (!info || info.agentName !== this.agent.name || info.sessionId !== requestedSession
        || !Number.isSafeInteger(info.runId)) {
        throw new CommandTransportError("Invalid send response identity");
      }
      // 压缩可能已通过事件切换身份，迟到的响应只补模型元信息。
      if (info.model !== undefined) {
        this.patch({ model: info.model ?? this.agent.provider ?? null, isCustomModel: Boolean(info.isCustomModel) });
      }
      return true;
    } catch (error) {
      if (!(error instanceof CommandTransportError)) {
        // 命令明确拒绝时保留草稿；其他客户端刚启动的运行不能被这次回滚清除。
        this.dispatch({ type: "reject_user", key,
          running: (this.identity?.runId ?? 0) > baseline && this.value.chat.running });
        this.awaitingRun = null;
        throw error;
      }
      // 帧可能先于传输失败到达；已观察到的新 run 不应被回滚。
      if ((this.identity?.runId ?? 0) > baseline) return true;
      this.uncertainSend = { runId: baseline, key };
      this.patch({ operation: "uncertain" });
      try {
        const info = await this.transport.invoke("session_info", {
          agentName: this.agent.name, sessionId: this.requireSessionId(),
        });
        if (info?.agentName === this.agent.name && info.sessionId === this.value.sessionId
          && (info.runId ?? 0) > baseline) {
          await this.hydrate();
          if (!this.uncertainSend) return true;
        }
      } catch { /* 查询失败不代表原请求失败；继续保留消息和停止入口。 */ }
      if (!this.uncertainSend) return true;
      this.fail("连接中断，暂时无法确认发送结果。消息已保留，不会自动重发；恢复连接后会同步，也可点击停止。");
      return false;
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
        const info = await this.transport.invoke("session_info", {
          agentName: this.agent.name, sessionId: this.requireSessionId(),
        });
        if (epoch !== this.runEpoch || this.getSnapshot().operation !== "stopping") return;
        if (!info || info.agentName !== this.agent.name || info.sessionId !== this.value.sessionId) {
          throw new Error("Unable to confirm the stopped session");
        }
        if (!info.running) {
          await this.hydrate();
          if (epoch !== this.runEpoch) return;
          if (this.value.load !== "ready") throw new Error("Unable to synchronize the stopped session");
          // 查询与水合之间可能出现新 run；停止旧 run 不能把新 run 标成完成。
          if (!this.uncertainSend && this.value.chat.running
            && (this.identity?.runId ?? 0) > (info.runId ?? 0)) {
            this.patch({ operation: "idle" });
            return;
          }
          if (this.uncertainSend) this.dispatch({ type: "reject_user", key: this.uncertainSend.key });
          this.uncertainSend = null;
          this.awaitingRun = null;
          this.settledRun = this.identity?.runId ?? this.settledRun;
          this.dispatch({ type: "event", event: { type: "agent_end" }, key: createEntryKey("stop") });
          this.patch({ operation: "idle", approval: null });
          return;
        }
        if (attempt === 50) throw new Error("停止请求已发送，但后端仍在运行；请稍后重试");
        await this.delay(100);
      }
    } catch (error) {
      if (epoch === this.runEpoch) { this.patch({ operation: this.uncertainSend ? "uncertain" : "idle" }); this.fail(error); }
    }
  }

  async resolveApproval(decision: ApprovalDecisionValue): Promise<void> {
    const request = this.value.approval;
    if (!request) return;
    this.patch({ approval: null });
    await this.transport.invoke("resolve_approval", { requestId: request.requestId, decision });
  }
  async compact(): Promise<void> {
    if (this.uncertainSend) throw new Error("Please confirm or stop the pending run first");
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
