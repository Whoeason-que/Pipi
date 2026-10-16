import assert from "node:assert/strict";
import test from "node:test";
import { DraftStore, submitDraft, type DraftStorage } from "../src/drafts.ts";
import { filterQuickItems, quickItems } from "../src/app/navigation.ts";
import { findMessages } from "../src/chat/find.ts";
import { groupChatBlocks } from "../src/chat-runtime.ts";
import type { AgentDefinition } from "../src/types.ts";

function storage(): DraftStorage {
  const values = new Map<string, string>();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); } };
}

test("drafts survive refresh and remain isolated by Agent, session and temporary mode", () => {
  const cache = storage();
  const store = new DraftStore(cache);
  store.get("a", "one").write("第一份草稿");
  store.get("a", "two").write("第二份草稿");
  store.get("b", "one").write("另一个 Agent");
  store.get("a", null).write("未创建会话");
  store.get("a", "one", true).write("临时测试");
  const restored = new DraftStore(cache);
  assert.equal(restored.get("a", "one").getSnapshot().text, "第一份草稿");
  assert.equal(restored.get("a", "two").getSnapshot().text, "第二份草稿");
  assert.equal(restored.get("b", "one").getSnapshot().text, "另一个 Agent");
  assert.equal(restored.get("a", null).getSnapshot().text, "未创建会话");
  assert.equal(restored.get("a", "one", true).getSnapshot().text, "");
});

test("identity migration keeps the buffer and frees the next new-session draft", async () => {
  const cache = storage();
  const store = new DraftStore(cache);
  const original = store.get("a", null);
  original.write("创建会话");
  let acknowledge!: (accepted: boolean) => void;
  const sending = submitDraft(original, () => new Promise(resolve => { acknowledge = resolve; }));
  store.move("a", null, "created", false);
  const nextDraft = store.get("a", null);
  nextDraft.write("下一条会话");
  assert.equal(store.get("a", "created"), original);
  acknowledge(true);
  await sending;
  assert.equal(original.getSnapshot().text, "");
  assert.equal(nextDraft.getSnapshot().text, "下一条会话");
  original.write("压缩时正在编辑");
  store.move("a", "created", "compacted", false);
  assert.equal(store.get("a", "compacted"), original);
  const refreshed = new DraftStore(cache);
  assert.equal(refreshed.get("a", "created").getSnapshot().text, "");
  assert.equal(refreshed.get("a", "compacted").getSnapshot().text, "压缩时正在编辑");
});

test("rejected and failed submissions preserve the exact input", async () => {
  const store = new DraftStore();
  const draft = store.get("a", "s");
  draft.write("  需要保留的内容\n");
  assert.equal(await submitDraft(draft, async () => false), false);
  await assert.rejects(submitDraft(draft, async () => { throw new Error("offline"); }), /offline/);
  assert.equal(draft.getSnapshot().text, "  需要保留的内容\n");
  assert.equal(await submitDraft(draft, async text => { assert.equal(text, "需要保留的内容"); return true; }), true);
  assert.equal(draft.getSnapshot().text, "");
});

test("late acknowledgement cannot clear newer typing or another session", async () => {
  const store = new DraftStore();
  const draft = store.get("a", "s");
  draft.write("已提交");
  let resolve!: (value: boolean) => void;
  const pending = submitDraft(draft, () => new Promise(done => { resolve = done; }));
  draft.write("后来输入");
  store.get("a", "other").write("另一个会话");
  resolve(true);
  await pending;
  assert.equal(draft.getSnapshot().text, "后来输入");
  assert.equal(store.get("a", "other").getSnapshot().text, "另一个会话");
});

test("archive/delete removes durable drafts without affecting other Agents", () => {
  const cache = storage();
  const store = new DraftStore(cache);
  store.get("a", "s").write("删除会话");
  store.get("a", "t").write("归档 Agent");
  store.get("a", "test", true).write("临时草稿");
  store.get("b", "s").write("保持");
  store.forget("a", "s");
  assert.equal(new DraftStore(cache).get("a", "s").getSnapshot().text, "");
  store.forgetAgent("a");
  assert.equal(store.get("a", "test", true).getSnapshot().text, "");
  const refreshed = new DraftStore(cache);
  assert.equal(refreshed.get("a", "t").getSnapshot().text, "");
  assert.equal(refreshed.get("b", "s").getSnapshot().text, "保持");
});

test("unavailable or malformed storage still allows in-memory editing and sending", async () => {
  const store = new DraftStore({ getItem: () => "broken", setItem: () => { throw new Error("quota"); } });
  const draft = store.get("a", "s");
  draft.write("本地内容");
  assert.equal(store.get("a", "s").getSnapshot().text, "本地内容");
  await submitDraft(draft, async () => true);
  assert.equal(draft.getSnapshot().text, "");
});

const agents = [
  { name: "coder", description: "开发项目" },
  { name: "reviewer", description: "审查代码" },
] as AgentDefinition[];

test("quick actions find Agents, cross-Agent sessions and contextual navigation", () => {
  const items = quickItems(agents, {
    coder: [{ id: "old", title: "修复 CSS", lastActive: 1, startedAt: 1, messageCount: 2 }],
    reviewer: [{ id: "new", title: "审查 CSS", lastActive: 10, startedAt: 10, messageCount: 3 }],
  }, "coder");
  assert.equal(items[0].target.kind, "action");
  assert.equal(filterQuickItems(items, "开发项目")[0].target.kind, "agent");
  assert.deepEqual(filterQuickItems(items, "CSS").map(item => item.label), ["审查 CSS", "修复 CSS"]);
  assert.equal(filterQuickItems(items, "REVIEWER css")[0].label, "审查 CSS");
  assert.equal(filterQuickItems(items, "missing").length, 0);
  assert.equal(quickItems(agents, {}, null).some(item => item.id === "new-session"), false);
});

test("conversation find searches body and code per message while excluding tools, thinking and system notices", () => {
  const blocks = groupChatBlocks([
    { key: "u", role: "user", text: "修复 Rust" },
    { key: "a", role: "assistant", text: "```rust\nfn main() {}\n```", thinking: "秘密思考" },
    { key: "t", role: "toolResult", toolName: "read", toolCallId: "x", text: "秘密工具 main" },
    { key: "r", role: "assistant", text: "main 通知", kind: "retry" },
  ]);
  assert.deepEqual(findMessages(blocks, "RUST"), ["u", "a"]);
  assert.deepEqual(findMessages(blocks, "main"), ["a"]);
  assert.deepEqual(findMessages(blocks, "秘密"), []);
  assert.deepEqual(findMessages(blocks, "  "), []);
});
