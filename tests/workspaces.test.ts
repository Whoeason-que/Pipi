import assert from "node:assert/strict";
import test from "node:test";
import {
  buildWorkspaceGroups,
  buildWorkspaceList,
  getWorkspaceBasename,
  normalizeWorkspacePath,
} from "../src/app/workspaces.ts";
import type { AgentDefinition } from "../src/types.ts";

function mockAgent(name: string, workspace: string | null): AgentDefinition {
  return {
    name,
    description: "",
    model: "test-model",
    workspace,
    permissions: {
      tools: [],
      sandbox: "workspace-write",
    },
    mcpServers: [],
    subagent: false,
    compactThresholdPercent: 75,
    systemPrompt: "both",
  };
}

test("normalizeWorkspacePath 规范化路径与去除尾斜杠", () => {
  assert.equal(normalizeWorkspacePath("/home/user/project/"), "/home/user/project");
  assert.equal(normalizeWorkspacePath("/home/user/project"), "/home/user/project");
  assert.equal(normalizeWorkspacePath("C:\\Users\\code\\project\\"), "C:\\Users\\code\\project");
  assert.equal(normalizeWorkspacePath("/"), "/");
  assert.equal(normalizeWorkspacePath(""), "");
  assert.equal(normalizeWorkspacePath(null), "");
  assert.equal(normalizeWorkspacePath("  /workspace/dir  "), "/workspace/dir");
});

test("getWorkspaceBasename 提取文件夹名称", () => {
  assert.equal(getWorkspaceBasename("/home/user/Desktop/code/Pipi"), "Pipi");
  assert.equal(getWorkspaceBasename("/home/user/my-project/"), "my-project");
  assert.equal(getWorkspaceBasename("C:\\code\\project-x"), "project-x");
});

test("buildWorkspaceList 合并 Agent 工作区与自定义工作区并计算计数", () => {
  const agents: AgentDefinition[] = [
    mockAgent("agent1", "/home/user/projectA"),
    mockAgent("agent2", "/home/user/projectA/"),
    mockAgent("agent3", "/home/user/projectB"),
    mockAgent("agent4", null), // 默认工作区
  ];

  const customWorkspaces = [
    { path: "/home/user/projectC", name: "Custom Project C" },
    { path: "/home/user/projectA" }, // 已存在的路径
  ];

  const list = buildWorkspaceList(agents, customWorkspaces);

  // 应该有 projectA, projectB, projectC
  assert.equal(list.length, 3);

  const itemA = list.find((w) => w.path === "/home/user/projectA");
  assert.ok(itemA);
  assert.equal(itemA.agentCount, 2);

  const itemB = list.find((w) => w.path === "/home/user/projectB");
  assert.ok(itemB);
  assert.equal(itemB.agentCount, 1);

  const itemC = list.find((w) => w.path === "/home/user/projectC");
  assert.ok(itemC);
  assert.equal(itemC.agentCount, 0);
  assert.equal(itemC.name, "Custom Project C");
  assert.equal(itemC.custom, true);
});

test("buildWorkspaceList 仅统计主 Agent 工作区，忽略 subagent", () => {
  const agent1 = mockAgent("agent1", "/home/user/projectA");
  const subagent1 = mockAgent("subagent1", "/home/user/projectSub");
  subagent1.subagent = true;

  const subagent2 = mockAgent("subagent2", "/home/user/projectA");
  subagent2.subagent = true;

  const list = buildWorkspaceList([agent1, subagent1, subagent2], []);

  // projectSub 不应该被添加，projectA 的 agentCount 应该仅为 1（不计入 subagent2）
  assert.equal(list.length, 1);
  assert.equal(list[0].path, "/home/user/projectA");
  assert.equal(list[0].agentCount, 1);
});

test("buildWorkspaceGroups 按工作区分组并合并归一化后的同路径", () => {
  const agents = [
    mockAgent("agent1", "/home/user/projectA"),
    mockAgent("agent2", "/home/user/projectA/"),
    mockAgent("agent3", "/home/user/projectB"),
  ];

  const groups = buildWorkspaceGroups(agents, []);

  assert.equal(groups.length, 2);
  const a = groups.find((g) => g.path === "/home/user/projectA");
  assert.ok(a);
  assert.deepEqual(a.agents.map((agent) => agent.name), ["agent1", "agent2"]);
  assert.equal(a.custom, false);
});

test("buildWorkspaceGroups 忽略 subagent，无工作区 Agent 进兜底组", () => {
  const main = mockAgent("main", "/home/user/projectA");
  const sub = mockAgent("sub", "/home/user/projectA");
  sub.subagent = true;
  const floating = mockAgent("floating", null);

  const groups = buildWorkspaceGroups([main, sub, floating], []);

  assert.equal(groups.length, 2);
  const ungrouped = groups.find((g) => g.path === "");
  assert.ok(ungrouped);
  assert.deepEqual(ungrouped.agents.map((agent) => agent.name), ["floating"]);
});

test("buildWorkspaceGroups 自定义目录保留为空组并排在有 Agent 的组之后", () => {
  const agents = [mockAgent("agent1", "/home/user/projectA")];
  const custom = [{ path: "/home/user/empty-dir/", name: "Empty" }];

  const groups = buildWorkspaceGroups(agents, custom);

  // 自定义组保留用户命名；空组沉底
  assert.equal(groups.length, 2);
  assert.equal(groups[0].path, "/home/user/projectA");
  const empty = groups[1];
  assert.equal(empty.path, "/home/user/empty-dir");
  assert.equal(empty.name, "Empty");
  assert.equal(empty.custom, true);
  assert.equal(empty.agents.length, 0);
});
