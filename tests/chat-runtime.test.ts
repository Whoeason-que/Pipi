import assert from "node:assert/strict";
import test from "node:test";
import {
  chatReducer,
  createEntryKey,
  eventMatchesRun,
  eventMatchesSession,
  INITIAL_CHAT_STATE,
  formatRuntimeError,
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
  // 载荷即 envelope：不再有裸事件形状，身份直接来自 payload
  assert.equal(payload.event.type, "agent_end");
  assert.equal(eventMatchesSession(payload, "coder", { sessionId: "session-2", runId: 3 }), true);
  assert.equal(eventMatchesSession(payload, "coder", { sessionId: "session-1", runId: 3 }), false);
  assert.equal(eventMatchesSession(payload, "coder", { sessionId: "session-2", runId: 4 }), false);
  assert.equal(eventMatchesSession({ ...payload, agentName: "other" }, "coder", null), false);
  assert.equal(eventMatchesRun(payload, "coder", { sessionId: "session-2", runId: 3 }, 3), false);
  assert.equal(eventMatchesRun(payload, "coder", { sessionId: "session-2", runId: 3 }, 2), true);
  assert.equal(eventMatchesRun({ ...payload, runId: 4 }, "coder", { sessionId: "session-2", runId: 3 }, 3), true);

  const stats = {
    agentName: "coder",
    sessionId: "session-2",
    runId: 3,
    stats: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0, calls: 1 },
  };
  assert.equal(stats.stats.calls, 1);
  assert.equal(stats.sessionId, "session-2");
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

test("rehydration with a retry notice acknowledges persisted messages once and keeps live entry keys", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, { type: "hydrate", messages: [{ role: "user", content: "history" }], stats: null, running: false });
  state = chatReducer(state, { type: "submit_user", key: "new-user", text: "new" });
  state = event(state, { type: "retry_start", attempt: 1, maxAttempts: 3, delayMs: 10, cause: "fixture" });
  state = event(state, { type: "message_end", message: assistantMessage("reply", "stop") });
  state = event(state, { type: "agent_end" });
  const replyKey = state.entries.find(entry => entry.text === "reply")!.key;
  const messages: MessageView[] = [{ role: "user", content: "history" }, { role: "user", content: "new" }, assistantMessage("reply", "stop")];
  state = chatReducer(state, { type: "hydrate", messages, stats: null, running: false });
  state = chatReducer(state, { type: "hydrate", messages, stats: null, running: false });
  assert.deepEqual(state.entries.filter(entry => !entry.kind).map(entry => entry.text), ["history", "new", "reply"]);
  assert.equal(state.entries.find(entry => entry.text === "reply")!.key, replyKey);
  assert.equal(state.entries.find(entry => entry.text === "new")!.key, "new-user");
  assert.equal(state.entries[2]?.kind, "retry");
});

test("repeated hydration keeps earlier run notices before later turns and preserves tool details", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, {
    type: "hydrate", messages: [{ role: "user", content: "history" }], stats: null, running: false,
  });
  state = chatReducer(state, { type: "submit_user", key: "stopped-user", text: "stop this run" });
  state = event(state, { type: "message_end", message: assistantMessage("interrupted", "aborted") });
  state = event(state, { type: "agent_end" });
  state = chatReducer(state, { type: "submit_user", key: "next-user", text: "next run" });
  state = event(state, { type: "tool_execution_start", toolCallId: "read-next", toolName: "read", args: { path: "README.md" } });
  state = event(state, {
    type: "tool_execution_end", toolCallId: "read-next", toolName: "read",
    result: { content: [{ type: "text", text: "read result" }], details: { path: "README.md" } }, isError: false,
  });
  state = event(state, { type: "retry_start", attempt: 1, maxAttempts: 3, delayMs: 10, cause: "fixture" });
  state = event(state, { type: "message_end", message: assistantMessage("completed", "stop") });
  state = event(state, { type: "agent_end" });
  const keys = state.entries.map(entry => entry.key);
  const messages: MessageView[] = [
    { role: "user", content: "history" },
    { role: "user", content: "stop this run" },
    { role: "user", content: "next run" },
    { role: "toolResult", toolCallId: "read-next", toolName: "read", content: "read result" },
    assistantMessage("completed", "stop"),
  ];
  for (let pass = 0; pass < 3; pass += 1) {
    state = chatReducer(state, { type: "hydrate", messages, stats: null, running: false });
    assert.deepEqual(state.entries.map(entry => entry.key), keys);
    assert.deepEqual(state.entries.filter(entry => !entry.kind).map(entry => entry.text),
      ["history", "stop this run", "interrupted", "next run", "read result", "completed"]);
    assert.equal(state.entries[5]?.kind, "retry");
    assert.deepEqual(state.entries.find(entry => entry.toolCallId === "read-next")?.toolArgs, { path: "README.md" });
    assert.deepEqual(state.entries.find(entry => entry.toolCallId === "read-next")?.toolDetails, { path: "README.md" });
  }
});

test("agent end does not duplicate assistant message with tool calls", () => {
  let state = chatReducer(INITIAL_CHAT_STATE, {
    type: "submit_user",
    key: "user-1",
    text: "run tool",
  });

  const toolMessage: MessageView = {
    role: "assistant",
    content: [
      { type: "text", text: "依赖图已拿到。接下来统计每个 crate 的代码量..." },
      { type: "toolCall", id: "call-1", name: "bash", arguments: {} },
    ],
    stopReason: "toolUse",
  };

  state = event(state, { type: "message_start", message: toolMessage });
  state = event(state, { type: "message_end", message: toolMessage });

  const finalMessage: MessageView = {
    role: "assistant",
    content: [{ type: "text", text: "quant 工作区统计报告" }],
    stopReason: "stop",
  };

  state = event(state, { type: "message_start", message: finalMessage });
  state = event(state, { type: "message_end", message: finalMessage });

  assert.deepEqual(
    state.entries.map((e) => e.text),
    [
      "run tool",
      "依赖图已拿到。接下来统计每个 crate 的代码量...",
      "quant 工作区统计报告",
    ],
  );

  // agent_end 带回本轮全部 messages，不应产生末尾幽灵重复条目
  state = event(state, {
    type: "agent_end",
    messages: [toolMessage, finalMessage],
  });

  assert.deepEqual(
    state.entries.map((e) => e.text),
    [
      "run tool",
      "依赖图已拿到。接下来统计每个 crate 的代码量...",
      "quant 工作区统计报告",
    ],
  );
});

test("classifyChatError correctly identifies all error categories with actionable suggestions", async () => {
  const { classifyChatError } = await import("../src/chat/chat-errors.ts");

  // 用户主动中止
  const aborted = classifyChatError({ status: "aborted", text: "■ 已中止" });
  assert.equal(aborted.category, "aborted");
  assert.equal(aborted.badge, "已中止");

  // 认证与 API Key
  const auth1 = classifyChatError({ errorMessage: "401 Unauthorized: Invalid API Key" });
  assert.equal(auth1.category, "auth");
  assert.equal(auth1.badge, "认证异常");
  assert.ok(auth1.suggestion?.includes("设置 → 模型服务"));

  const auth2 = classifyChatError({ errorMessage: "ProviderError::InvalidAuthentication" });
  assert.equal(auth2.category, "auth");

  // 频率与配额限制
  const rateLimit = classifyChatError({ errorMessage: "429 Too Many Requests: Rate limit exceeded for model" });
  assert.equal(rateLimit.category, "rate_limit");
  assert.equal(rateLimit.badge, "频率限制");
  assert.ok(rateLimit.suggestion?.includes("稍候片刻"));

  const quota = classifyChatError({ errorMessage: "insufficient_quota: 账户余额已耗尽" });
  assert.equal(quota.category, "rate_limit");

  // 上下文超限
  const context = classifyChatError({ errorMessage: "maximum context length is 128000 tokens, but prompt had 135000" });
  assert.equal(context.category, "context_overflow");
  assert.equal(context.badge, "上下文超限");
  assert.ok(context.suggestion?.includes("会话压缩"));

  // 协议不匹配
  const protocol = classifyChatError({ errorMessage: "ModelProtocolUnsupported: Anthropic / OpenAI 两种协议均已尝试" });
  assert.equal(protocol.category, "protocol");
  assert.equal(protocol.badge, "协议错误");
  assert.ok(protocol.suggestion?.includes("自动"));

  // 权限拦截
  const permission = classifyChatError({ errorMessage: "命令被拒绝：检测到安全策略拦截" });
  assert.equal(permission.category, "permission");
  assert.equal(permission.badge, "权限拦截");

  // 网络超时与中断
  const timeout = classifyChatError({ errorMessage: "请求超时（timeout after 30s）：流提前结束" });
  assert.equal(timeout.category, "network");
  assert.equal(timeout.badge, "连接异常");
  assert.ok(timeout.suggestion?.includes("代理"));

  // 通用错误
  const generic = classifyChatError({ errorMessage: "500 Internal Server Error in backend cluster" });
  assert.equal(generic.category, "generic");
  assert.equal(generic.badge, "运行错误");
});

test("extractAssistantContentAndError separates prose and error states cleanly", async () => {
  const { extractAssistantContentAndError } = await import("../src/chat/chat-errors.ts");

  // 1. 正常正文，无错误
  const normal = extractAssistantContentAndError({
    text: "这是生成的回答",
  });
  assert.equal(normal.cleanText, "这是生成的回答");
  assert.equal(normal.errorInfo, null);

  // 2. 纯错误文本
  const pureError = extractAssistantContentAndError({
    text: "⚠ 401 Unauthorized",
    status: "error",
    errorMessage: "401 Unauthorized",
  });
  assert.equal(pureError.cleanText, "");
  assert.equal(pureError.errorInfo?.category, "auth");

  // 3. 半截正文 + 中止标记
  const partialAborted = extractAssistantContentAndError({
    text: "代码实现如下：\n```ts\nconst x = 1;\n```\n■ 已中止",
    status: "aborted",
    stopReason: "aborted",
  });
  assert.equal(partialAborted.cleanText, "代码实现如下：\n```ts\nconst x = 1;\n```");
  assert.equal(partialAborted.errorInfo?.category, "aborted");

  // 4. 纯中止
  const pureAborted = extractAssistantContentAndError({
    text: "■ 已中止",
    status: "aborted",
  });
  assert.equal(pureAborted.cleanText, "");
  assert.equal(pureAborted.errorInfo?.category, "aborted");

  // 5. 输出部分正文后遭遇网络超时
  const partialNetworkError = extractAssistantContentAndError({
    text: "第一步先解析 AST：",
    status: "error",
    errorMessage: "流提前结束：未收到结束帧（连接被中断）",
  });
  assert.equal(partialNetworkError.cleanText, "第一步先解析 AST：");
  assert.equal(partialNetworkError.errorInfo?.category, "network");
});


