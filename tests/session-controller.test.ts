import type { CommandName, CommandArgs, CommandResult, EventMap } from "../src/ipc.ts";
import assert from "node:assert/strict";
import test from "node:test";
import type { AgentEvent, MessageView } from "../src/chat-runtime.ts";
import { SessionRuntime, type RuntimeTransport } from "../src/session-runtime.ts";
import type { AgentDefinition, SessionInfoView, SessionStatsView } from "../src/types.ts";

const agent: AgentDefinition = {
  name: "fixture", description: "", model: "", provider: null, workspace: null,
  permissions: { tools: ["read"], bash: { mode: "allowlist", commands: [] }, sandbox: "workspace-write" },
  mcpServers: [], subagent: false, compactThresholdPercent: 75,
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
  async invoke<K extends CommandName>(command: K, ...parameters: CommandArgs<K>): Promise<CommandResult<K>> {
    const args = parameters[0] as Record<string, unknown> | undefined;
    this.calls.push({ command, args });
    const session = String(args?.sessionId ?? "s");
    let value: unknown;
    switch (command) {
      case "session_messages": value = this.held ? await this.held : [...this.history]; break;
      case "session_stats": value = stats; break;
      case "session_info": value = this.infos.get(session) ?? null; break;
      case "session_running": value = this.infos.get(session)?.running ?? false; break;
      case "stop_run": this.infos.set(session, { ...this.infos.get(session)!, running: false }); break;
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

test("a draft claims the server identity even when agent_start follows the IPC response", async () => {
  const { host, runtime } = await fixture();
  const draft = runtime.get(agent, null);
  await draft.hydrate();
  await draft.send("create");
  host.event({ type: "agent_start" }, "created", 3);
  assert.equal(draft.getSnapshot().sessionId, "created");
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
  const held = deferred<boolean>();
  const original = host.invoke.bind(host);
  host.invoke = async (command, ...args) => command === "session_running"
    ? await held.promise as never : original(command, ...args);
  const stopping = controller.stop();
  await Promise.resolve();
  host.event({ type: "agent_end" }, "s", 2);
  await controller.send("next run");
  host.event({ type: "agent_start" }, "s", 3);
  held.resolve(false);
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
