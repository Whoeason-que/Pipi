import assert from "node:assert/strict";
import test from "node:test";
import {
  chatReducer,
  createEntryKey,
  eventMatchesRun,
  eventMatchesSession,
  INITIAL_CHAT_STATE,
  formatRuntimeError,
  normalizeAgentEvent,
  normalizeStatsPayload,
  type AgentEvent,
  type MessageView,
} from "../src/chat-runtime.ts";

const assistantMessage = (text: string, stopReason?: string): MessageView => ({
  role: "assistant",
  content: [{ type: "text", text }],
  stopReason,
});

function event(state: typeof INITIAL_CHAT_STATE, value: AgentEvent) {
  return chatReducer(state, { type: "event", event: value, key: createEntryKey("test") });
}

test("hydration preserves optimistic entries that arrive before the snapshot", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, {
    type: "submit_user",
    key: "user-local",
    text: "最新问题",
  });

  state = chatReducer(state, {
    type: "hydrate",
    messages: [{ role: "user", content: "历史问题" }],
    stats: null,
    running: true,
  });

  assert.deepEqual(state.entries.map((entry) => entry.text), ["历史问题", "最新问题"]);
  assert.equal(state.running, true);
  assert.equal(state.hydrated, true);
});

test("hydration does not duplicate messages already acknowledged by persistence", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, {
    type: "submit_user",
    key: "user-local",
    text: "同一条问题",
  });
  state = chatReducer(state, {
    type: "hydrate",
    messages: [{ role: "user", content: "同一条问题" }],
    stats: null,
    running: false,
  });
  state = event(state, { type: "message_end", message: assistantMessage("已完成", "stop") });
  state = chatReducer(state, {
    type: "hydrate",
    messages: [
      { role: "user", content: "同一条问题" },
      assistantMessage("已完成", "stop"),
    ],
    stats: null,
    running: false,
  });
  assert.deepEqual(state.entries.map((entry) => entry.text), ["同一条问题", "已完成"]);
});
test("hydration keeps a new duplicate-text user turn distinct from history", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, {
    type: "hydrate",
    messages: [{ role: "user", content: "相同问题" }],
    stats: null,
    running: false,
  });
  state = chatReducer(state, { type: "submit_user", key: "user-new", text: "相同问题" });
  state = chatReducer(state, {
    type: "hydrate",
    messages: [{ role: "user", content: "相同问题" }],
    stats: null,
    running: false,
  });
  assert.deepEqual(state.entries.map((entry) => entry.text), ["相同问题", "相同问题"]);

  state = chatReducer(state, {
    type: "hydrate",
    messages: [
      { role: "user", content: "相同问题" },
      { role: "user", content: "相同问题" },
    ],
    stats: null,
    running: false,
  });
  assert.equal(state.entries.length, 2);
});
test("late hydration cannot resurrect a settled live run", () => {
  let state = INITIAL_CHAT_STATE;
  state = event(state, { type: "agent_start" });
  state = event(state, { type: "agent_end" });
  state = chatReducer(state, { type: "hydrate", messages: [], stats: null, running: true });
  assert.equal(state.running, false);
});
test("streaming assistant messages update in place and duplicate completion is ignored", () => {
  let state = event(INITIAL_CHAT_STATE, { type: "agent_start" });
  state = event(state, { type: "message_start", message: assistantMessage("") });
  state = event(state, { type: "message_update", message: assistantMessage("部分回答") });
  state = event(state, { type: "message_end", message: assistantMessage("完整回答", "stop") });
  state = event(state, { type: "message_end", message: assistantMessage("完整回答", "stop") });

  assert.equal(state.entries.length, 1);
  assert.equal(state.entries[0]?.text, "完整回答");
  assert.equal(state.entries[0]?.streaming, false);
  assert.equal(state.entries[0]?.status, undefined);
});

test("tool execution is idempotent by tool call id", () => {
  let state = event(INITIAL_CHAT_STATE, {
    type: "tool_execution_start",
    toolCallId: "call-1",
    toolName: "read",
  });
  state = event(state, {
    type: "tool_execution_start",
    toolCallId: "call-1",
    toolName: "read",
  });
  state = event(state, {
    type: "tool_execution_update",
    toolCallId: "call-1",
    toolName: "read",
    partial: { content: [{ type: "text", text: "partial" }] },
  });
  state = event(state, {
    type: "tool_execution_end",
    toolCallId: "call-1",
    toolName: "read",
    result: { content: [{ type: "text", text: "done" }] },
    isError: false,
  });
  state = event(state, {
    type: "message_start",
    message: {
      role: "toolResult",
      toolCallId: "call-1",
      toolName: "read",
      content: [{ type: "text", text: "done" }],
    },
  });
  state = event(state, {
    type: "message_end",
    message: {
      role: "toolResult",
      toolCallId: "call-1",
      toolName: "read",
      content: [{ type: "text", text: "done" }],
    },
  });

  assert.equal(state.entries.length, 1);
  assert.equal(state.entries[0]?.text, "done");
  assert.equal(state.entries[0]?.toolRunning, false);
});

test("agent end finalizes every incomplete live entry", () => {
  let state = event(INITIAL_CHAT_STATE, {
    type: "message_start",
    message: assistantMessage("还没结束"),
  });
  state = event(state, {
    type: "tool_execution_start",
    toolCallId: "call-2",
    toolName: "bash",
  });
  state = event(state, { type: "agent_end" });

  assert.equal(state.running, false);
  assert.equal(state.entries.every((entry) => !entry.streaming && !entry.toolRunning), true);
  assert.equal(state.entries.every((entry) => entry.status === "aborted"), true);
});

test("agent end recovers messages when the view missed the stream", () => {
  let state = event(INITIAL_CHAT_STATE, { type: "agent_start" });
  state = event(state, {
    type: "agent_end",
    messages: [assistantMessage("切换期间完成的回答", "stop")],
  });

  assert.equal(state.running, false);
  assert.equal(state.entries.length, 1);
  assert.equal(state.entries[0]?.text, "切换期间完成的回答");
  assert.equal(state.entries[0]?.streaming, false);
});

test("session envelopes reject stale identity and unwrap stats", () => {
  const payload = {
    agentName: "coder",
    sessionId: "session-2",
    runId: 3,
    event: { type: "agent_end" } as const,
  };
  const normalized = normalizeAgentEvent(payload);
  assert.equal(normalized.event.type, "agent_end");
  assert.equal(eventMatchesSession(normalized.meta, "coder", { sessionId: "session-2", runId: 3 }), true);
  assert.equal(eventMatchesSession(normalized.meta, "coder", { sessionId: "session-1", runId: 3 }), false);
  assert.equal(eventMatchesSession(normalized.meta, "coder", { sessionId: "session-2", runId: 4 }), false);
  assert.equal(eventMatchesRun(normalized.meta, "coder", { sessionId: "session-2", runId: 3 }, 3), false);
  assert.equal(eventMatchesRun(normalized.meta, "coder", { sessionId: "session-2", runId: 3 }, 2), true);
  assert.equal(eventMatchesRun({ ...payload, runId: 4 }, "coder", { sessionId: "session-2", runId: 3 }, 3), true);

  const stats = normalizeStatsPayload({
    agentName: "coder",
    sessionId: "session-2",
    runId: 3,
    stats: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0, calls: 1 },
  });
  assert.equal(stats.stats.calls, 1);
  assert.equal(stats.meta?.sessionId, "session-2");
});

test("runtime errors are normalized without stringifying objects", () => {
  assert.equal(formatRuntimeError(new Error("failed")), "failed");
  assert.equal(formatRuntimeError({ message: "rejected" }), "rejected");
  assert.equal(formatRuntimeError("plain"), "plain");
  assert.equal(formatRuntimeError(null), "未知错误");
});

test("thinking content is extracted and updated during streaming", () => {
  let state = event(INITIAL_CHAT_STATE, { type: "agent_start" });
  state = event(state, {
    type: "message_start",
    message: {
      role: "assistant",
      content: [{ type: "thinking", thinking: "正在思考..." }],
    },
  });
  assert.equal(state.entries.length, 1);
  assert.equal(state.entries[0]?.thinking, "正在思考...");

  state = event(state, {
    type: "message_update",
    message: {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "正在思考...想通了" },
        { type: "text", text: "你好！" },
      ],
    },
  });
  assert.equal(state.entries[0]?.thinking, "正在思考...想通了");
  assert.equal(state.entries[0]?.text, "你好！");
});

test("tool execution preserves toolArgs and toolDetails across lifecycle", () => {
  let state = event(INITIAL_CHAT_STATE, {
    type: "tool_execution_start",
    toolCallId: "call-edit-1",
    toolName: "edit",
    args: { path: "src/main.rs", edits: [{ oldText: "a", newText: "b" }] },
  });
  assert.deepEqual(state.entries[0]?.toolArgs, {
    path: "src/main.rs",
    edits: [{ oldText: "a", newText: "b" }],
  });

  state = event(state, {
    type: "tool_execution_end",
    toolCallId: "call-edit-1",
    toolName: "edit",
    result: {
      content: [{ type: "text", text: "已修改" }],
      details: { diff: "@@ -1 +1 @@\n-a\n+b", firstChangedLine: 1 },
    },
    isError: false,
  });
  assert.deepEqual(state.entries[0]?.toolArgs, {
    path: "src/main.rs",
    edits: [{ oldText: "a", newText: "b" }],
  });
  assert.deepEqual(state.entries[0]?.toolDetails, {
    diff: "@@ -1 +1 @@\n-a\n+b",
    firstChangedLine: 1,
  });
});


test("compaction events produce a single system entry with collapsible summary", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, {
    type: "submit_user",
    key: "user-local",
    text: "开始干活",
  });

  state = event(state, { type: "compaction_start" });
  assert.equal(state.running, true);
  const pendingKey = state.activeCompactionKey;
  assert.ok(pendingKey);
  assert.equal(state.entries.at(-1)?.kind, "compaction");
  assert.equal(state.entries.at(-1)?.text, "♻ 正在压缩上下文…");

  state = event(state, { type: "compaction_end", summary: "## 摘要\n- 修好了 bug", replaced: 12 });
  assert.equal(state.activeCompactionKey, null);
  // 进行中条目被原位替换，而不是追加第二条
  assert.equal(state.entries.filter((entry) => entry.kind === "compaction").length, 1);
  assert.equal(state.entries.at(-1)?.text, "♻ 已压缩上下文：12 条旧消息已并入摘要");
  assert.equal(state.entries.at(-1)?.summary, "## 摘要\n- 修好了 bug");

  // agent_end 清理压缩态
  state = event(state, { type: "agent_end" });
  assert.equal(state.activeCompactionKey, null);
  assert.equal(state.running, false);
});
