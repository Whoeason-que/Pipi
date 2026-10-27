import assert from "node:assert/strict";
import test from "node:test";
import { NavigationMemory } from "../src/app/navigation-memory.ts";
import type { SessionController } from "../src/session-controller.ts";
import type { AgentDefinition } from "../src/types.ts";

const agents = [
  { name: "main", subagent: false },
  { name: "helper", subagent: true },
] as AgentDefinition[];
const controller = {} as SessionController;

test("Agent 和 Subagent 分区分别恢复上次页面，临时页面不覆盖记忆", () => {
  const memory = new NavigationMemory();
  const chat = { kind: "chat", agentName: "main", sessionId: "one" } as const;
  const workbench = { kind: "workbench", agentName: "helper" } as const;
  memory.remember(chat, agents);
  memory.remember(workbench, agents);
  memory.remember({ kind: "files", agentName: null }, agents);
  memory.remember({ kind: "settings" }, agents);
  assert.deepEqual(memory.restore("agents", agents), chat);
  assert.deepEqual(memory.restore("subagents", agents), workbench);
  assert.deepEqual(memory.restore("agents", [agents[1]]), { kind: "home", section: "agents" });
  assert.deepEqual(memory.restore("agents", agents), chat, "过滤工作区不会抹掉原记忆");
});

test("后台换会话 ID、归档和删除只更新对应 Agent 的记忆", () => {
  const memory = new NavigationMemory();
  memory.remember({ kind: "chat", agentName: "main", sessionId: "old" }, agents);
  memory.remember({ kind: "chat", agentName: "helper", sessionId: "old" }, agents);
  assert.equal(memory.rekey("main", "old", "new", controller), true);
  assert.deepEqual(memory.restore("agents", agents), { kind: "chat", agentName: "main", sessionId: "new" });
  assert.deepEqual(memory.restore("subagents", agents), { kind: "chat", agentName: "helper", sessionId: "old" });
  memory.forgetSession("main", "old");
  assert.equal(memory.restore("agents", agents).kind, "chat");
  memory.forgetSession("main", "new");
  assert.deepEqual(memory.restore("agents", agents), { kind: "agent", agentName: "main" });
  memory.forgetAgent("helper");
  assert.deepEqual(memory.restore("subagents", agents), { kind: "home", section: "subagents" });
});

test("新会话的迟到身份不会覆盖同一 Agent 随后创建的另一个草稿", () => {
  const memory = new NavigationMemory();
  const earlier = {} as SessionController;
  const later = {} as SessionController;
  memory.remember({ kind: "chat", agentName: "main", sessionId: null }, agents);
  memory.bindController("main", null, earlier);
  memory.unbindDraft("main");
  memory.remember({ kind: "chat", agentName: "main", sessionId: null }, agents);
  memory.bindController("main", null, later);
  assert.equal(memory.rekey("main", null, "earlier", earlier), false);
  assert.equal(memory.controllerFor("main", null), later);
  assert.equal(memory.rekey("main", null, "later", later), true);
  assert.deepEqual(memory.restore("agents", agents), { kind: "chat", agentName: "main", sessionId: "later" });
});
