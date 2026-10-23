import type { CommandName, CommandArgs, CommandResult, EventMap } from "../src/ipc.ts";
import assert from "node:assert/strict";
import test from "node:test";
import type { AgentEvent, MessageView } from "../src/chat-runtime.ts";
import { SessionRuntime, type RuntimeTransport } from "../src/session-runtime.ts";
import { CommandTransportError, decodeCommandResponse } from "../src/transport-error.ts";
import { submitDraft } from "../src/drafts.ts";
import type { AgentDefinition, SessionInfoView, SessionStatsView } from "../src/types.ts";

const agent: AgentDefinition = {
  name: "fixture", description: "", model: "", provider: null, workspace: null,
  permissions: { tools: ["read"], bash: { mode: "allowlist", commands: [] }, sandbox: "workspace-write" },
  mcpServers: [], subagent: false, compactThresholdPercent: 75, systemPrompt: "both",
};
const stats: SessionStatsView = { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, calls: 1 };
const message = (text: string): MessageView => ({ role: "assistant", content: text });
const info = (sessionId = "s", runId = 1, running = false): SessionInfoView => ({
  agentName: agent.name, sessionId, runId, running, temporary: false,
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

class Host implements RuntimeTransport {
  calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
  infos = new Map<string, SessionInfoView>([["s", info()]]);
  history: MessageView[] = [];
  held: Promise<MessageView[]> | null = null;
  /** send_prompt 响应附加字段（模型元信息等）；默认不带 model，避免影响其他用例。 */
  sendInfo: Partial<SessionInfoView> = {};
  /** 非空时 send_prompt 的响应挂起，供「事件先于响应」的用例控制顺序。 */
  sendGate: Promise<void> | null = null;
  async invoke<K extends CommandName>(command: K, ...parameters: CommandArgs<K>): Promise<CommandResult<K>> {
    const args = parameters[0] as Record<string, unknown> | undefined;
    this.calls.push({ command, args });
    const session = String(args?.sessionId ?? "s");
    let value: unknown;
    switch (command) {
      case "open_session": {
        const currentInfo = this.infos.get(session) ?? info(session);
        const messages = this.held ? await this.held : [...this.history];
        value = { info: currentInfo, messages, stats };
        break;
      }
      case "session_messages": value = this.held ? await this.held : [...this.history]; break;
      case "session_stats": value = stats; break;
      case "session_info": value = this.infos.get(session) ?? null; break;
      case "stop_run": this.infos.set(session, { ...this.infos.get(session)!, running: false }); break;
      case "create_session": {
        const created = info("created", 0, false);
        this.infos.set("created", created);
        value = created;
        break;
      }
      case "send_prompt": {
        // 与核心同构：不带 sessionId = 新会话，响应返回它的权威身份。
        const target = args?.sessionId === null || args?.sessionId === undefined
          ? "created" : String(args.sessionId);
        const snapshot = args?.sessionId === "created" ? info(target, 3, true)
          : this.infos.get(target) ?? info(target, 3, true);
        this.infos.set(target, snapshot);
        if (this.sendGate) await this.sendGate;
        value = { ...snapshot, ...this.sendInfo };
        break;
      }
    }
    return value as CommandResult<K>;
  }
  async listen<K extends keyof EventMap>(name: K, listener: (event: { payload: EventMap[K] }) => void): Promise<() => void> {
    const listeners = this.listeners.get(name) ?? new Set();
    const callback = listener as (event: { payload: unknown }) => void;
    listeners.add(callback);
    this.listeners.set(name, listeners);
    return () => { listeners.delete(callback); };
  }
  event(event: AgentEvent, sessionId = "s", runId = 2): void {
    this.emit("agent-event", { agentName: agent.name, sessionId, runId, event });
  }
  emit(name: string, payload: unknown): void {
    this.listeners.get(name)?.forEach(listener => listener({ payload }));
  }
}

async function fixture() {
  const host = new Host();
  const runtime = new SessionRuntime(host);
  await runtime.start();
  return { host, runtime, controller: runtime.get(agent, "s") };
}

test("transport recovery hydrates missed completion and clears obsolete approvals without stopping runs", async () => {
  const { host, runtime, controller } = await fixture();
  const unsubscribe = controller.subscribe(() => {});
  host.infos.set("s", info("s", 2, true));
  await controller.hydrate();
  host.emit("approval-request", { agentName: agent.name, sessionId: "s", runId: 2,
    requestId: "pending", command: "pwd", missing: [] });
  assert.ok(controller.getSnapshot().approval);
  host.infos.set("s", info("s", 2, false));
  host.history = [message("completed while disconnected")];
  await runtime.resynchronize();
  assert.equal(controller.getSnapshot().chat.running, false);
  assert.equal(controller.getSnapshot().approval, null);
  assert.ok(controller.getSnapshot().chat.entries.some(entry => entry.text === "completed while disconnected"));
  assert.equal(host.calls.some(call => call.command === "stop_run"), false);
  unsubscribe(); runtime.disconnect();
});

test("loading merges events that arrive before history and does not resurrect a completed run", async () => {
  const { host, controller, runtime } = await fixture();
  const history = deferred<MessageView[]>();
  host.held = history.promise;
  const hydration = controller.hydrate();
  host.event({ type: "agent_start" });
  host.event({ type: "message_update", message: message("live") });
  host.event({ type: "agent_end", messages: [message("live")] });
  history.resolve([{ role: "user", content: "history" }]);
  await hydration;
  assert.equal(controller.getSnapshot().load, "ready");
  assert.deepEqual(controller.getSnapshot().chat.entries.map(entry => entry.text), ["history", "live"]);
  assert.equal(controller.getSnapshot().chat.running, false);
  runtime.disconnect();
});

test("a late old completion cannot settle the new run or replay stale text", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  await controller.send("new");
  host.event({ type: "agent_end", messages: [message("old")] }, "s", 1);
  assert.equal(controller.getSnapshot().chat.running, true);
  host.event({ type: "agent_start" }, "s", 2);
  host.event({ type: "agent_end", messages: [message("new reply")] }, "s", 2);
  assert.deepEqual(controller.getSnapshot().chat.entries.map(entry => entry.text), ["new", "new reply"]);
  runtime.disconnect();
});

test("repeated user text remains a separate optimistic turn", async () => {
  const { host, controller, runtime } = await fixture();
  host.history = [{ role: "user", content: "same" }, message("previous")];
  await controller.hydrate();
  await controller.send("same");
  host.event({ type: "agent_start" });
  host.event({ type: "message_end", message: { role: "user", content: "same" } });
  assert.equal(controller.getSnapshot().chat.entries.filter(entry => entry.role === "user").length, 2);
  runtime.disconnect();
});

test("view unsubscribe keeps background events and never calls stop_run", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const unsubscribe = controller.subscribe(() => {});
  host.event({ type: "agent_start" });
  unsubscribe();
  host.event({ type: "agent_end", messages: [message("offscreen")] });
  assert.equal(runtime.get(agent, "s"), controller);
  assert.equal(controller.getSnapshot().chat.entries.at(-1)?.text, "offscreen");
  assert.equal(host.calls.some(call => call.command === "stop_run"), false);
  runtime.disconnect();
});

test("send_prompt's SessionInfo establishes a new draft's identity and model", async () => {
  const { host, runtime } = await fixture();
  host.sendInfo = { model: { id: "m1", name: "M1", api: "openai-completions", baseUrl: "", maxTokens: 8192, contextWindow: 200000 }, isCustomModel: true };
  const draft = runtime.get(agent, null);
  await draft.hydrate();
  await draft.send("create");
  assert.equal(draft.getSnapshot().sessionId, "created");
  assert.equal(runtime.get(agent, "created"), draft);
  assert.equal(draft.getSnapshot().model?.id, "m1");
  assert.equal(draft.getSnapshot().isCustomModel, true);
  runtime.disconnect();
});

test("an agent_start that beats the IPC response is claimed by the draft, and the response only fills metadata", async () => {
  const { host, runtime } = await fixture();
  const gate = deferred<void>();
  host.sendGate = gate.promise;
  host.sendInfo = { model: { id: "m1", name: "M1", api: "openai-completions", baseUrl: "", maxTokens: 8192, contextWindow: 200000 }, isCustomModel: true };
  const draft = runtime.get(agent, null);
  await draft.hydrate();
  const sending = draft.send("create");
  await Promise.resolve(); // 稳定会话身份先于发送建立。
  // 事件先到：按 Agent 名认领（响应还挂着）
  host.event({ type: "agent_start" }, "created", 3);
  assert.equal(draft.getSnapshot().sessionId, "created");
  // 响应后到：身份不变，补上模型元信息
  gate.resolve();
  assert.equal(await sending, true);
  assert.equal(draft.getSnapshot().sessionId, "created");
  assert.equal(draft.getSnapshot().model?.id, "m1");
  assert.equal(runtime.get(agent, "created"), draft);
  runtime.disconnect();
});

test("compaction rekeys the same controller and stop targets the new session", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  host.event({ type: "agent_start" });
  host.infos.set("compacted", info("compacted", 2, true));
  host.emit("session-switched", { agentName: agent.name, sessionId: "s", runId: 2,
    toSessionId: "compacted", archived: true });
  assert.equal(runtime.get(agent, "compacted"), controller);
  await controller.stop();
  assert.equal(host.calls.find(call => call.command === "stop_run")?.args?.sessionId, "compacted");
  assert.equal(controller.getSnapshot().chat.running, false);
  runtime.disconnect();
});

test("late hydration results cannot replace a newer snapshot", async () => {
  const { host, controller, runtime } = await fixture();
  const first = deferred<MessageView[]>();
  host.held = first.promise;
  const previous = controller.hydrate();
  host.held = null;
  host.history = [message("new snapshot")];
  await controller.hydrate();
  first.resolve([message("stale snapshot")]);
  await previous;
  assert.deepEqual(controller.getSnapshot().chat.entries.map(entry => entry.text), ["new snapshot"]);
  runtime.disconnect();
});

test("a compaction switch during hydration replays the new identity's queued completion", async () => {
  const { host, controller, runtime } = await fixture();
  host.infos.set("s", info("s", 2, true));
  host.infos.set("compacted", info("compacted", 2, false));
  const first = deferred<MessageView[]>();
  host.held = first.promise;
  const hydration = controller.hydrate();
  host.emit("session-switched", { agentName: agent.name, sessionId: "s", runId: 2,
    toSessionId: "compacted", archived: true });
  host.event({ type: "agent_end", messages: [message("after compaction")] }, "compacted", 2);
  first.resolve([]);
  await hydration;
  assert.equal(runtime.get(agent, "compacted"), controller);
  assert.equal(controller.getSnapshot().chat.running, false);
  assert.equal(controller.getSnapshot().chat.entries.at(-1)?.text, "after compaction");
  runtime.disconnect();
});

test("missing or mismatched identity fails closed and cannot send", async () => {
  const { host, controller, runtime } = await fixture();
  host.infos.set("s", { ...info(), agentName: "other" });
  await controller.hydrate();
  assert.equal(controller.getSnapshot().load, "failed");
  assert.equal(await controller.send("blocked"), false);
  assert.equal(host.calls.some(call => call.command === "send_prompt"), false);
  runtime.disconnect();
});

test("sessions of the same Agent receive isolated events and share one host subscription", async () => {
  const { host, runtime, controller } = await fixture();
  await runtime.start();
  host.infos.set("other", info("other"));
  const other = runtime.get(agent, "other");
  await Promise.all([controller.hydrate(), other.hydrate()]);
  host.event({ type: "agent_start" });
  host.event({ type: "message_update", message: message("only first") });
  assert.equal(other.getSnapshot().chat.entries.length, 0);
  assert.equal(other.getSnapshot().chat.running, false);
  assert.equal([...host.listeners.values()].reduce((count, listeners) => count + listeners.size, 0), 6);
  runtime.disconnect();
  assert.equal([...host.listeners.values()].reduce((count, listeners) => count + listeners.size, 0), 0);
});

test("idle cache limits never evict running sessions or impose a concurrency limit", async () => {
  const { host, runtime } = await fixture();
  const controllers = [];
  for (let index = 0; index < 40; index++) {
    const id = `parallel-${index}`;
    host.infos.set(id, info(id, 2, true));
    const controller = runtime.get(agent, id);
    await controller.hydrate();
    controllers.push({ id, controller });
  }
  for (const { id, controller } of controllers) assert.equal(runtime.get(agent, id), controller);
  runtime.disconnect();
});

test("a stale backend snapshot cannot downgrade a known run and accept its old completion", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  host.event({ type: "agent_start" }, "s", 3);
  await controller.hydrate(); // host snapshot still says idle run 1
  assert.equal(controller.getSnapshot().chat.running, true);
  host.event({ type: "agent_end", messages: [message("stale")] }, "s", 1);
  assert.equal(controller.getSnapshot().chat.running, true);
  assert.equal(controller.getSnapshot().chat.entries.length, 0);
  host.event({ type: "agent_end", messages: [message("current")] }, "s", 3);
  assert.equal(controller.getSnapshot().chat.running, false);
  runtime.disconnect();
});

test("a late stop poll cannot finish a newly started run", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  host.event({ type: "agent_start" }, "s", 2);
  const held = deferred<SessionInfoView | null>();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => command === "session_info"
    ? await held.promise as never : original(command, ...args);
  const stopping = controller.stop();
  await Promise.resolve();
  host.event({ type: "agent_end" }, "s", 2);
  await controller.send("next run");
  host.event({ type: "agent_start" }, "s", 3);
  held.resolve(info("s", 2, false));
  await stopping;
  assert.equal(controller.getSnapshot().chat.running, true);
  assert.equal(controller.getSnapshot().operation, "idle");
  runtime.disconnect();
});

test("old approval and error frames cannot cross the run boundary", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  host.event({ type: "agent_start" }, "s", 3);
  host.emit("approval-request", { agentName: agent.name, sessionId: "s", runId: 2, requestId: "old", command: "ls", missing: ["ls"] });
  host.emit("session-error", { agentName: agent.name, sessionId: "s", runId: 2, message: "old" });
  assert.equal(controller.getSnapshot().approval, null);
  assert.equal(controller.getSnapshot().error, null);
  host.emit("approval-request", { agentName: agent.name, sessionId: "s", runId: 3, requestId: "current", command: "ls", missing: ["ls"] });
  assert.equal(controller.getSnapshot().approval?.requestId, "current");
  host.event({ type: "agent_end" }, "s", 3);
  assert.equal(controller.getSnapshot().approval, null);
  runtime.disconnect();
});

test("a failed host subscription cleans partial listeners and can reconnect without duplicates", async () => {
  const host = new Host();
  const original = host.listen.bind(host);
  let fail = true;
  host.listen = async (name, listener) => {
    if (fail && name === "session-error") throw new Error("fixture subscription failure");
    return original(name, listener);
  };
  const runtime = new SessionRuntime(host);
  await assert.rejects(runtime.start(), /fixture subscription failure/);
  assert.equal([...host.listeners.values()].reduce((count, listeners) => count + listeners.size, 0), 0);
  fail = false;
  await runtime.start();
  assert.equal([...host.listeners.values()].reduce((count, listeners) => count + listeners.size, 0), 6);
  runtime.disconnect();
  assert.equal([...host.listeners.values()].reduce((count, listeners) => count + listeners.size, 0), 0);
});

test("composer drafts follow authoritative creation and compaction without crossing concurrent sessions", async () => {
  const { host, runtime } = await fixture();
  const draft = runtime.composerDrafts.get(agent.name, null);
  draft.write("first prompt");
  const controller = runtime.get(agent, null);
  await controller.hydrate();
  await submitDraft(draft, text => controller.send(text));
  assert.equal(runtime.composerDrafts.get(agent.name, "created"), draft);
  draft.write("next steering draft");
  runtime.composerDrafts.get(agent.name, "peer").write("peer draft");
  host.emit("session-switched", { agentName: agent.name, sessionId: "created", runId: 3, toSessionId: "compact" });
  assert.equal(runtime.composerDrafts.get(agent.name, "compact"), draft);
  assert.equal(draft.getSnapshot().text, "next steering draft");
  assert.equal(runtime.composerDrafts.get(agent.name, "peer").getSnapshot().text, "peer draft");
  runtime.forget(agent.name, "compact");
  assert.equal(draft.getSnapshot().text, "");
  assert.equal(runtime.composerDrafts.get(agent.name, "peer").getSnapshot().text, "peer draft");
});


test("a lost response recovers an accepted run without dropping the prompt or resending", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") {
      host.calls.push({ command });
      host.infos.set("s", info("s", 2, true));
      throw new CommandTransportError("response lost");
    }
    return original(command, ...args);
  };
  assert.equal(await controller.send("keep this prompt"), true);
  assert.equal(controller.getSnapshot().chat.running, true);
  assert.equal(controller.getSnapshot().operation, "idle");
  assert.ok(controller.getSnapshot().chat.entries.some(entry => entry.text === "keep this prompt"));
  assert.equal(host.calls.filter(call => call.command === "send_prompt").length, 1);
  await controller.stop();
  assert.equal(controller.getSnapshot().chat.running, false);
  runtime.disconnect();
});

test("an unknown send keeps the draft and stop control, and blocks duplicate sends or steering", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  let requests = 0;
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") { requests++; throw new CommandTransportError("offline"); }
    if (command === "session_info") throw new CommandTransportError("still offline");
    return original(command, ...args);
  };
  const draft = runtime.composerDrafts.get(agent.name, "s");
  draft.write("do not lose me");
  assert.equal(await submitDraft(draft, text => controller.send(text)), false);
  assert.equal(draft.getSnapshot().text, "do not lose me");
  assert.equal(controller.getSnapshot().operation, "uncertain");
  assert.equal(controller.getSnapshot().chat.running, true);
  assert.equal(await controller.send("another prompt"), false);
  assert.equal(requests, 1);
  await controller.hydrate(); // 旧的 idle 快照不能证明发送失败。
  assert.equal(controller.getSnapshot().chat.running, true);
  host.invoke = original;
  await controller.stop();
  assert.equal(controller.getSnapshot().chat.running, false);
  assert.equal(controller.getSnapshot().operation, "idle");
  assert.equal(draft.getSnapshot().text, "do not lose me");
  assert.equal(controller.getSnapshot().chat.entries.some(entry => entry.text === "do not lose me"), false);
  runtime.disconnect();
});

test("a late completion resolves an unknown send and preserves partial error content", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") throw new CommandTransportError("response lost");
    return original(command, ...args);
  };
  assert.equal(await controller.send("hello"), false);
  const partial: MessageView = { role: "assistant", content: "already generated",
    stopReason: "error", errorMessage: "stream closed", errorKind: "network" };
  host.event({ type: "agent_end", messages: [partial] }, "s", 2);
  assert.equal(controller.getSnapshot().chat.running, false);
  assert.equal(controller.getSnapshot().operation, "idle");
  assert.equal(controller.getSnapshot().error, null);
  assert.equal(controller.getSnapshot().chat.entries.at(-1)?.text, "already generated");
  assert.equal(controller.getSnapshot().chat.entries.at(-1)?.errorKind, "network");
  runtime.disconnect();
});

test("a confirmed rejection restores the draft and removes only the rejected prompt", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") throw "Invalid model configuration";
    return original(command, ...args);
  };
  const draft = runtime.composerDrafts.get(agent.name, "s");
  draft.write("retry after fixing configuration");
  await assert.rejects(submitDraft(draft, text => controller.send(text)), error => error === "Invalid model configuration");
  assert.equal(draft.getSnapshot().text, "retry after fixing configuration");
  assert.equal(controller.getSnapshot().chat.entries.length, 0);
  assert.equal(controller.getSnapshot().chat.running, false);
  runtime.disconnect();
});

test("a new draft reserves identity before sending and cannot start a run if reservation fails", async () => {
  const { host, runtime } = await fixture();
  const controller = runtime.get(agent, null);
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "create_session") throw new CommandTransportError("reservation response lost");
    return original(command, ...args);
  };
  await assert.rejects(controller.send("new prompt"), /reservation response lost/);
  assert.equal(host.calls.some(call => call.command === "send_prompt"), false);
  assert.equal(controller.getSnapshot().chat.running, false);
  host.invoke = original;
  await controller.send("new prompt");
  assert.equal(host.calls.find(call => call.command === "send_prompt")?.args?.sessionId, "created");
  runtime.disconnect();
});


test("HTTP command rejections stay distinct from proxy failures and corrupt responses", () => {
  assert.throws(() => decodeCommandResponse(400, '{"error":"Invalid model"}'), error =>
    error instanceof Error && !(error instanceof CommandTransportError) && error.message === "Invalid model");
  for (const [status, body] of [[502, "bad gateway"], [504, '{"error":"timeout"}'], [200, "incomplete json"], [400, "bad gateway"]] as const) {
    assert.throws(() => decodeCommandResponse(status, body), CommandTransportError);
  }
  assert.equal(decodeCommandResponse(200, "null"), null);
});


test("a rejected prompt cannot clear another observed run or consume the draft", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") {
      host.event({ type: "agent_start" }, "s", 2);
      throw "Session is already running";
    }
    return original(command, ...args);
  };
  await assert.rejects(controller.send("rejected prompt"), error => error === "Session is already running");
  assert.equal(controller.getSnapshot().chat.running, true);
  assert.equal(controller.getSnapshot().chat.entries.length, 0);
  runtime.disconnect();
});

test("an advanced but idle run number alone cannot acknowledge an uncertain prompt", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") {
      // 核心在准备失败时也可能已分配 runId；没有正文/用户账本不能证明成功。
      host.infos.set("s", info("s", 2, false));
      throw new CommandTransportError("response lost");
    }
    return original(command, ...args);
  };
  assert.equal(await controller.send("still a draft"), false);
  assert.equal(controller.getSnapshot().operation, "uncertain");
  assert.equal(controller.getSnapshot().chat.running, true);
  await controller.stop();
  assert.equal(controller.getSnapshot().chat.entries.length, 0);
  runtime.disconnect();
});

test("a completed send can be acknowledged from its persisted user prompt after response loss", async () => {
  const { host, controller, runtime } = await fixture();
  await controller.hydrate();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => {
    if (command === "send_prompt") {
      host.infos.set("s", info("s", 2, false));
      host.history = [{ role: "user", content: "finished quickly" }, message("done")];
      throw new CommandTransportError("response lost");
    }
    return original(command, ...args);
  };
  assert.equal(await controller.send("finished quickly"), true);
  assert.equal(controller.getSnapshot().chat.running, false);
  assert.equal(controller.getSnapshot().operation, "idle");
  assert.deepEqual(controller.getSnapshot().chat.entries.map(entry => entry.text), ["finished quickly", "done"]);
  runtime.disconnect();
});


test("stopping one run does not suppress a later run from the host", async () => {
  const { host, controller, runtime } = await fixture();
  host.infos.set("s", info("s", 2, true));
  await controller.hydrate();
  await controller.stop();
  host.event({ type: "agent_start" }, "s", 3);
  assert.equal(controller.getSnapshot().chat.running, true);
  host.event({ type: "agent_end" }, "s", 2);
  assert.equal(controller.getSnapshot().chat.running, true);
  runtime.disconnect();
});

test("stop cannot settle a newer host run that starts during final hydration", async () => {
  const { host, controller, runtime } = await fixture();
  host.infos.set("s", info("s", 2, true));
  await controller.hydrate();
  const history = deferred<MessageView[]>();
  host.held = history.promise;
  const stopping = controller.stop();
  for (let attempt = 0; attempt < 10 && controller.getSnapshot().load !== "loading"; attempt++) await Promise.resolve();
  assert.equal(controller.getSnapshot().load, "loading");
  host.event({ type: "agent_start" }, "s", 3);
  history.resolve([]);
  await stopping;
  assert.equal(controller.getSnapshot().chat.running, true);
  assert.equal(controller.getSnapshot().operation, "idle");
  runtime.disconnect();
});
