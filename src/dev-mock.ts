// 仅开发环境生效：在普通浏览器（无 Tauri 后端）里提供最小 invoke 桩，
// 让前端 UI 可以脱离桌面壳独立开发调试。Tauri 生产构建不受影响。
import type { AgentDefinition, Settings } from "./types";
import type { ModelCatalog } from "./catalog";

const demoAgent: AgentDefinition = {
  name: "demo-assistant",
  description: "浏览器演示模式的示例 Agent（真实 Agent 由桌面端读写 ~/.pipi/agents）",
  model: "",
  provider: null,
  workspace: null,
  permissions: {
    tools: ["read", "write", "edit", "bash", "memory", "glob", "grep"],
    bash: { mode: "allowAll", commands: [] },
    sandbox: "workspace-write",
  },
  mcpServers: [],
  compactThresholdPercent: 75,
};

const initialDemoMessages: Array<Record<string, unknown>> = [
  { role: "user", content: "帮我看看这个项目的结构", timestamp: Date.now() - 260000 },
  {
    role: "assistant",
    content: [{ type: "text", text: "我先用 bash 看一下目录。" }],
    usage: { input: 40, output: 48, cacheRead: 280, cacheWrite: 0, totalTokens: 368 },
    stopReason: "toolUse",
    timestamp: Date.now() - 240000,
    durationMs: 1900,
  },
  { role: "toolResult", toolCallId: "t1", toolName: "bash", content: [{ type: "text", text: "src/ README.md" }], isError: false, timestamp: Date.now() - 238000 },
  {
    role: "assistant",
    content: [{ type: "text", text: `项目结构很简洁，核心三块：

- **src/** — React + TS 前端（零 UI 框架）
- **src-tauri/** — Tauri 薄壳，命令只做 IPC 转发
- **crates/pipi-core/** — 从 pi 移植的 Agent 内核

主循环的调用方式：

\`\`\`rust
let messages = run_agent_loop(
    vec![Message::user_text("hi")],
    context,
    config,
    emitter,
    abort,
).await;
\`\`\`

| 模块 | 职责 |
| --- | --- |
| agent_loop | 工具调用循环 |
| provider | rig 适配层 |

> 一切皆文件，配置即代码。详见 README。` }],
    usage: { input: 30, output: 96, cacheRead: 380, cacheWrite: 0, totalTokens: 506 },
    stopReason: "stop",
    timestamp: Date.now() - 230000,
    durationMs: 2600,
  },
];

const initialDemoSessionId = "1730000000000-abc123";
const legacyDemoSessionId = "1729990000000-def456";
const peerDemoSessionId = "1729980000000-ghi789";

/** 第二个演示 Agent：用来演示「两个 Agent 各跑一轮」（运行态是按 Agent 的集合）。 */
const demoPeer: AgentDefinition = {
  name: "demo-reviewer",
  description: "浏览器演示模式的第二个 Agent：演示多 Agent 并行运行",
  model: "",
  provider: null,
  workspace: null,
  permissions: {
    tools: ["read", "glob", "grep"],
    bash: { mode: "allowAll", commands: [] },
    sandbox: "read-only",
  },
  mcpServers: [],
  compactThresholdPercent: 75,
};

const peerDemoMessages: Array<Record<string, unknown>> = [
  { role: "user", content: "审查一下 src/ 的组件拆分", timestamp: Date.now() - 60000 },
  {
    role: "assistant",
    content: [{ type: "text", text: "读完了 src/：App.tsx 负责编排，Chat.tsx 是单会话视图。需要我把拆分建议写下来吗？" }],
    usage: { input: 20, output: 40, cacheRead: 120, cacheWrite: 0, totalTokens: 180 },
    stopReason: "stop",
    timestamp: Date.now() - 58000,
  },
];

/**
 * 每个 Agent 一份会话状态 —— 与核心同构：核心是「一个 Agent 一个会话槽 +
 * 一个运行守卫」，不同 Agent 可以各跑一轮，所以 mock 不能再用单份模块级状态。
 */
/** 每个 Agent 的账本：它名下有哪些会话（历史、标题、归档区）。 */
interface DemoAgentState {
  histories: Map<string, Array<Record<string, unknown>>>;
  titles: Map<string, string>;
  archivedSessions: Map<string, Array<Record<string, unknown>>>;
}

function seedDemoState(
  seeds: Array<[string, string, Array<Record<string, unknown>>]>,
): DemoAgentState {
  const histories = new Map<string, Array<Record<string, unknown>>>();
  const titles = new Map<string, string>();
  for (const [id, title, messages] of seeds) {
    titles.set(id, title);
    histories.set(id, messages);
  }
  return { histories, titles, archivedSessions: new Map() };
}

let demoSessionCounter = 0;

const demoStates = new Map<string, DemoAgentState>([
  [
    demoAgent.name,
    seedDemoState([
      [initialDemoSessionId, "帮我看看这个项目的结构", initialDemoMessages],
      [
        legacyDemoSessionId,
        "把 README 翻译成英文",
        [
          { role: "user", content: "把 README 翻译成英文", timestamp: 0 },
          { role: "assistant", content: [{ type: "text", text: "好的，我会先读取 README。" }], timestamp: 0 },
        ],
      ],
    ]),
  ],
  [
    demoPeer.name,
    seedDemoState([[peerDemoSessionId, "审查一下 src/ 的组件拆分", peerDemoMessages]]),
  ],
]);

/** 取（必要时新建）某个 Agent 的账本。 */
function stateFor(agentName: string): DemoAgentState {
  const existing = demoStates.get(agentName);
  if (existing) return existing;
  const created = seedDemoState([]);
  demoStates.set(agentName, created);
  return created;
}

/** 一条**打开中**的会话 —— 与核心同构：同一个 Agent 可以同时开多条，各自独立运行。 */
interface DemoConversation {
  agentName: string;
  sessionId: string;
  temporary: boolean;
  messages: Array<Record<string, unknown>>;
  running: boolean;
  runId: number;
  runTimer: ReturnType<typeof setTimeout> | null;
}

const conversationKey = (agentName: string, sessionId: string) => `${agentName}\u0000${sessionId}`;

const demoConversations = new Map<string, DemoConversation>();

/** 打开（必要时按历史装载）一条会话；已打开则原样返回。 */
function openConversation(agentName: string, sessionId: string): DemoConversation {
  const key = conversationKey(agentName, sessionId);
  const existing = demoConversations.get(key);
  if (existing) return existing;
  const state = stateFor(agentName);
  const messages = state.histories.get(sessionId) ?? [];
  state.histories.set(sessionId, messages);
  const conversation: DemoConversation = {
    agentName,
    sessionId,
    temporary: false,
    messages,
    running: false,
    runId: 0,
    runTimer: null,
  };
  demoConversations.set(key, conversation);
  return conversation;
}

/** 设置工作台的临时测试会话：只进打开中 map，不进入 histories / 会话列表。 */
function createTestConversation(agentName: string): DemoConversation {
  demoSessionCounter += 1;
  const sessionId = `test-demo-${demoSessionCounter}-${Date.now().toString(36)}`;
  const conversation: DemoConversation = {
    agentName,
    sessionId,
    temporary: true,
    messages: [],
    running: false,
    runId: 0,
    runTimer: null,
  };
  demoConversations.set(conversationKey(agentName, sessionId), conversation);
  return conversation;
}

function testConversationOf(agentName: string): DemoConversation | undefined {
  return [...demoConversations.values()].find(
    (conversation) => conversation.agentName === agentName && conversation.temporary,
  );
}

/** 取一条已打开的会话（没打开返回 undefined）。 */
function conversationOf(agentName: string, sessionId: string): DemoConversation | undefined {
  return demoConversations.get(conversationKey(agentName, sessionId));
}

/** 新建一条会话（还没发消息的「新会话」也算）。 */
function createConversation(agentName: string): DemoConversation {
  const state = stateFor(agentName);
  demoSessionCounter += 1;
  const sessionId = `demo-session-${demoSessionCounter}-${Date.now().toString(36)}`;
  state.histories.set(sessionId, []);
  return openConversation(agentName, sessionId);
}

interface DevEvent {
  payload: unknown;
}

type DevEventListener = (event: DevEvent) => void;

const devListeners = new Map<string, Set<DevEventListener>>();
// 归档演示状态：Agent 可被「归档/恢复/删除」，在内存里挪动。
// deleted 与 archived 分开：删除是永久消失（对应真实后端删目录），
// 归档后仍可恢复（对应真实后端从 .archive 移回）。
const demoAgentArchived = new Set<string>();
const demoAgentDeleted = new Set<string>();

function emitDevEvent(name: string, payload: unknown): void {
  devListeners.get(name)?.forEach((listener) => listener({ payload }));
}

function emitDemoAgentEvent(conversation: DemoConversation, event: unknown): void {
  emitDevEvent("agent-event", {
    agentName: conversation.agentName,
    sessionId: conversation.sessionId,
    runId: conversation.runId,
    event,
  });
}

function listenDevEvent(name: string, listener: DevEventListener): () => void {
  const listeners = devListeners.get(name) ?? new Set<DevEventListener>();
  listeners.add(listener);
  devListeners.set(name, listeners);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) devListeners.delete(name);
  };
}

function startDemoRun(conversation: DemoConversation, prompt: string): void {
  const state = stateFor(conversation.agentName);
  if (conversation.runTimer) clearTimeout(conversation.runTimer);
  conversation.runId += 1;
  conversation.running = true;
  if (!conversation.temporary) {
    state.titles.set(
      conversation.sessionId,
      state.titles.get(conversation.sessionId) ?? prompt,
    );
  }
  conversation.messages.push({ role: "user", content: prompt, timestamp: Date.now() });

  // 演示剧本：thinking → bash（成功）→ thinking → bash（失败）→ 正文，
  // 覆盖标签行折叠、同名计数（thinking ×2 / bash ×2）与失败标红。
  const call1 = `call-demo-${conversation.runId}-1`;
  const call2 = `call-demo-${conversation.runId}-2`;
  const thinking1 = "先确认一下项目结构，再决定改哪里。";
  const thinking2 = "README 里应该有构建命令，直接读一下。";
  const toolCallBlock = (id: string, command: string) => ({
    type: "toolCall" as const,
    id,
    name: "bash",
    arguments: { command },
  });
  const toolResult = (id: string, text: string, isError = false) => ({
    role: "toolResult",
    toolName: "bash",
    toolCallId: id,
    content: [{ type: "toolResultText", text }],
    isError,
    timestamp: Date.now(),
  });
  const response = {
    role: "assistant",
    content: [{
      type: "text",
      text: `我已收到：${prompt}\n\n演示模式：工具调用与思考已折叠为正文上方的标签，悬浮可预览、点击固定到右侧「调用详情」。`,
    }],
    usage: { input: 60, output: 32, cacheRead: 440, cacheWrite: 0, totalTokens: 532 },
    stopReason: "stop",
    timestamp: Date.now(),
    durationMs: 320,
  };

  const steps: Array<() => void> = [
    () => emitDemoAgentEvent(conversation, { type: "agent_start" }),
    // 第 1 条助手消息：thinking + bash 调用
    () => emitDemoAgentEvent(conversation, { type: "message_start", message: { role: "assistant", content: [], timestamp: Date.now() } }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_update",
      message: { role: "assistant", content: [{ type: "thinking", thinking: thinking1 }] },
    }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_update",
      message: { role: "assistant", content: [{ type: "thinking", thinking: thinking1 }, toolCallBlock(call1, "ls -la")] },
    }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_end",
      message: { role: "assistant", content: [{ type: "thinking", thinking: thinking1 }, toolCallBlock(call1, "ls -la")], stopReason: "toolUse", timestamp: Date.now() },
    }),
    // 第 1 次 bash：运行中 → 部分输出 → 完成
    () => emitDemoAgentEvent(conversation, { type: "tool_execution_start", toolCallId: call1, toolName: "bash", args: { command: "ls -la" } }),
    () => emitDemoAgentEvent(conversation, {
      type: "tool_execution_update",
      toolCallId: call1,
      toolName: "bash",
      partial: { content: [{ type: "text", text: "total 24\ndrwxr-xr-x  src" }] },
    }),
    () => emitDemoAgentEvent(conversation, {
      type: "tool_execution_end",
      toolCallId: call1,
      toolName: "bash",
      result: { content: [{ type: "text", text: "README.md\nsrc/\ncrates/\npackage.json" }] },
      isError: false,
    }),
    () => emitDemoAgentEvent(conversation, { type: "message_start", message: toolResult(call1, "README.md\nsrc/\ncrates/\npackage.json") }),
    () => emitDemoAgentEvent(conversation, { type: "message_end", message: toolResult(call1, "README.md\nsrc/\ncrates/\npackage.json") }),
    // 第 2 条助手消息：thinking + 再次 bash（读取失败，验证失败标红）
    () => emitDemoAgentEvent(conversation, { type: "message_start", message: { role: "assistant", content: [], timestamp: Date.now() } }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_update",
      message: { role: "assistant", content: [{ type: "thinking", thinking: thinking2 }, toolCallBlock(call2, "cat CONTRIBUTING.md")] },
    }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_end",
      message: { role: "assistant", content: [{ type: "thinking", thinking: thinking2 }, toolCallBlock(call2, "cat CONTRIBUTING.md")], stopReason: "toolUse", timestamp: Date.now() },
    }),
    () => emitDemoAgentEvent(conversation, { type: "tool_execution_start", toolCallId: call2, toolName: "bash", args: { command: "cat CONTRIBUTING.md" } }),
    () => emitDemoAgentEvent(conversation, {
      type: "tool_execution_end",
      toolCallId: call2,
      toolName: "bash",
      result: { content: [{ type: "text", text: "cat: CONTRIBUTING.md: No such file or directory" }] },
      isError: true,
    }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_start",
      message: { ...toolResult(call2, "cat: CONTRIBUTING.md: No such file or directory", true), isError: true },
    }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_end",
      message: { ...toolResult(call2, "cat: CONTRIBUTING.md: No such file or directory", true), isError: true },
    }),
    // 第 3 条助手消息：正文回复（正文出现即断开标签行）
    () => emitDemoAgentEvent(conversation, { type: "message_start", message: { role: "assistant", content: [], timestamp: Date.now() } }),
    () => emitDemoAgentEvent(conversation, {
      type: "message_update",
      message: { ...response, content: [{ type: "text", text: `我已收到：${prompt}\n\n` }] },
    }),
    () => emitDemoAgentEvent(conversation, { type: "message_update", message: response }),
    () => emitDemoAgentEvent(conversation, { type: "message_end", message: response }),
    () => {
      conversation.messages.push(response);
      emitDevEvent("session-stats", {
        agentName: conversation.agentName,
        sessionId: conversation.sessionId,
        runId: conversation.runId,
        stats: {
          input: 130,
          output: 176,
          cacheRead: 1100,
          cacheWrite: 0,
          calls: 3,
          avgTps: 51.2,
          avgLatencyS: 1.8,
          cacheHitPct: 88.0,
          contextUsed: 500,
          contextMax: 200000,
          contextPercent: 0,
        },
      });
    },
    () => emitDemoAgentEvent(conversation, { type: "agent_end", messages: [response] }),
  ];

  let stepIndex = 0;
  const runNextStep = () => {
    if (stepIndex >= steps.length) {
      conversation.runTimer = null;
      conversation.running = false;
      return;
    }
    steps[stepIndex]();
    stepIndex += 1;
    conversation.runTimer = setTimeout(runNextStep, 240);
  };
  conversation.runTimer = setTimeout(runNextStep, 120);
}

interface DevPlatform {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen(event: string, listener: DevEventListener): Promise<() => void>;
}

interface TauriInternals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  transformCallback: (cb: unknown) => unknown;
}

/** 浏览器演示用的模型目录桩：结构与 pipi-core 的 models.dev 结果一致（camelCase）。 */
const demoCatalog: ModelCatalog = {
  fetchedAt: Math.floor(Date.now() / 1000),
  source: "models.dev",
  stale: false,
  providers: [
    {
      id: "anthropic",
      name: "Anthropic",
      api: "anthropic-messages",
      baseUrl: "https://api.anthropic.com",
      envKey: "ANTHROPIC_API_KEY",
      group: "国际",
      local: false,
      models: [
        { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", context: 1000000, output: 128000, reasoning: true },
        { id: "claude-opus-4-5", name: "Claude Opus 4.5", context: 200000, output: 64000, reasoning: true },
      ],
    },
    {
      id: "openai",
      name: "OpenAI",
      api: "openai-completions",
      baseUrl: "https://api.openai.com/v1",
      envKey: "OPENAI_API_KEY",
      group: "国际",
      local: false,
      models: [
        { id: "gpt-5", name: "GPT-5", context: 400000, output: 128000, reasoning: true },
        { id: "gpt-5-mini", name: "GPT-5 mini", context: 400000, output: 128000, reasoning: true },
      ],
    },
    {
      id: "deepseek",
      name: "DeepSeek",
      api: "openai-completions",
      baseUrl: "https://api.deepseek.com/v1",
      envKey: "DEEPSEEK_API_KEY",
      group: "国内",
      local: false,
      models: [
        { id: "deepseek-chat", name: "DeepSeek Chat", context: 128000, output: 8192 },
        { id: "deepseek-reasoner", name: "DeepSeek Reasoner", context: 128000, output: 65536, reasoning: true },
      ],
    },
    {
      id: "ollama",
      name: "Ollama（本地）",
      api: "openai-completions",
      baseUrl: "http://localhost:11434/v1",
      envKey: "OLLAMA_API_KEY",
      group: "本地",
      local: true,
      doc: "https://docs.ollama.com/api/openai-compatibility",
      note: "本地服务：密钥填任意非空值（如 ollama）；模型名以 `ollama list` 为准",
      models: [],
    },
  ],
};

const settings: Settings = {
  theme: localStorage.getItem("pipi-theme") === "light" ? "light" : "dark",
  providers: [
    {
      id: "anthropic",
      name: "Anthropic",
      api: "anthropic-messages",
      baseUrl: "https://api.anthropic.com",
      envKey: "ANTHROPIC_API_KEY",
      apiKey: null,
    },
    {
      id: "openai",
      name: "OpenAI",
      api: "openai-completions",
      baseUrl: "https://api.openai.com/v1",
      envKey: "OPENAI_API_KEY",
      apiKey: null,
    },
    {
      id: "deepseek",
      name: "DeepSeek",
      api: "openai-completions",
      baseUrl: "https://api.deepseek.com/v1",
      envKey: "DEEPSEEK_API_KEY",
      apiKey: null,
    },
  ],
  defaultProviderId: "anthropic",
  compaction: { forkBeforeCompact: true, archiveOriginal: true },
};

function listDemoSessions(agentName: string): Array<Record<string, unknown>> {
  const state = stateFor(agentName);
  return [...state.histories.entries()]
    .filter(([, messages]) => messages.length > 0)
    .map(([id, messages]) => ({
      id,
      title: state.titles.get(id) ?? "未命名会话",
      messageCount: messages.length,
      startedAt: Number(id.split("-")[0]) || 0,
      lastActive: Number(id.split("-")[0]) || 0,
    }));
}

export function installDevMock(): void {
  if (!import.meta.env.DEV) return;
  const w = window as unknown as {
    __TAURI_INTERNALS__?: TauriInternals;
    __PIPI_DEV_PLATFORM__?: DevPlatform;
  };
  if (w.__TAURI_INTERNALS__) return;
  const invoke = (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "get_settings":
          return Promise.resolve(structuredClone(settings));
        case "save_settings":
          Object.assign(settings, args.settings);
          return Promise.resolve(null);
        case "list_agents":
          // 尊重归档/删除状态：归档后活跃列表为空，删除后彻底消失
          // 尊重归档/删除状态：归档后活跃列表为空，删除后彻底消失
          return Promise.resolve(
            [demoAgent, demoPeer].filter(
              (agent) => !demoAgentArchived.has(agent.name) && !demoAgentDeleted.has(agent.name),
            ),
          );
        case "save_agent": {
          // 浏览器演示模式：把保存落回 demoAgent，让「改完刷新」的流程可验证
          const def = args.def as AgentDefinition | undefined;
          const current = def ? testConversationOf(def.name) : undefined;
          if (current?.running) {
            return Promise.reject(new Error("临时测试仍在运行，请先停止再保存设置"));
          }
          if (def && def.name === demoAgent.name) Object.assign(demoAgent, def);
          if (current) demoConversations.delete(conversationKey(current.agentName, current.sessionId));
          return Promise.resolve(null);
        }
        case "model_catalog":
          // 与 pipi-core 的 wire 结构一致（providers/models 按 camelCase）
          return Promise.resolve(structuredClone(demoCatalog));
        case "load_agent": {
          const name = String(args.name ?? "");
          return Promise.resolve(
            [demoAgent, demoPeer].find((agent) => agent.name === name) ?? demoAgent,
          );
        }
        case "list_agent_files":
          return Promise.resolve(["AGENTS.md", "memory/user-prefs.md"]);
        case "read_agent_file":
          return Promise.resolve(
            String(args.relPath) === "AGENTS.md"
              ? "# demo-assistant\n\n演示模式的系统指令。\n"
              : "",
          );
        case "write_agent_file": {
          const agentName = String(args.agentName ?? demoAgent.name);
          const current = testConversationOf(agentName);
          if (current?.running) {
            return Promise.reject(new Error("临时测试仍在运行，请先停止再保存文件"));
          }
          if (current) demoConversations.delete(conversationKey(agentName, current.sessionId));
          return Promise.resolve(null);
        }
        case "list_sessions":
          return Promise.resolve(listDemoSessions(String(args.agentName ?? demoAgent.name)));
        case "open_session": {
          const agentName = String(args.agentName ?? demoAgent.name);
          openConversation(agentName, String(args.sessionId ?? ""));
          return Promise.resolve(null);
        }
        case "session_info": {
          const agentName = String(args.agentName ?? demoAgent.name);
          const conversation = conversationOf(agentName, String(args.sessionId ?? ""));
          return Promise.resolve(
            conversation
              ? {
                  agentName,
                  sessionId: conversation.sessionId,
                  temporary: conversation.temporary,
                  running: conversation.running,
                  runId: conversation.runId,
                }
              : null,
          );
        }
        // 所有打开中的会话（核心 session_infos 的同构桩：同一个 Agent 可以有多条）
        case "session_infos":
          return Promise.resolve(
            [...demoConversations.values()].map((conversation) => ({
              agentName: conversation.agentName,
              sessionId: conversation.sessionId,
              temporary: conversation.temporary,
              running: conversation.running,
              runId: conversation.runId,
            })),
          );
        case "ensure_test_session": {
          const agentName = String(args.agentName ?? demoAgent.name);
          const conversation = testConversationOf(agentName) ?? createTestConversation(agentName);
          return Promise.resolve({
            agentName,
            sessionId: conversation.sessionId,
            temporary: true,
            running: conversation.running,
            runId: conversation.runId,
          });
        }
        case "reset_test_session": {
          const agentName = String(args.agentName ?? demoAgent.name);
          const current = testConversationOf(agentName);
          if (current?.running) {
            return Promise.reject(new Error("临时测试仍在运行，请先停止再清空或保存设置"));
          }
          if (current) demoConversations.delete(conversationKey(agentName, current.sessionId));
          const conversation = createTestConversation(agentName);
          return Promise.resolve({
            agentName,
            sessionId: conversation.sessionId,
            temporary: true,
            running: false,
            runId: 0,
          });
        }
        case "session_messages": {
          const conversation = conversationOf(
            String(args.agentName ?? demoAgent.name),
            String(args.sessionId ?? ""),
          );
          return Promise.resolve(conversation?.messages ?? []);
        }
        case "session_stats":
          return Promise.resolve({
            input: 70,
            output: 144,
            cacheRead: 660,
            cacheWrite: 0,
            calls: 2,
            avgTps: 53.8,
            avgLatencyS: 2.25,
            cacheHitPct: 92.7,
            contextUsed: 410,
            contextMax: 200000,
            contextPercent: 0,
          });
        case "session_running": {
          const conversation = conversationOf(
            String(args.agentName ?? demoAgent.name),
            String(args.sessionId ?? ""),
          );
          return Promise.resolve(Boolean(conversation?.running));
        }
        case "send_prompt": {
          const agentName = String(args.agentName ?? demoAgent.name);
          // 会话 id 为空 = 新会话（核心会创建文件，这里创建一条演示会话）
          const requested = args.sessionId === null || args.sessionId === undefined
            ? null
            : String(args.sessionId);
          const conversation = requested
            ? openConversation(agentName, requested)
            : createConversation(agentName);
          // 与核心同构：同一条会话不允许并发两轮（别的会话互不影响）
          if (conversation.running) {
            return Promise.reject(
              new Error(`会话「${conversation.sessionId}」正在运行，请等待完成或先停止`),
            );
          }
          startDemoRun(conversation, String(args.prompt ?? ""));
          return Promise.resolve(null);
        }
        case "compact_now":
          // 演示模式没有真实会话可压：返回拒绝的 Promise，让 UI 的错误通路走到
          return Promise.reject(new Error("演示模式不支持压缩上下文（接上核心后才可用）"));
        case "steer": {
          // 演示桩：把插话作为用户消息回显
          const conversation = conversationOf(
            String(args.agentName ?? demoAgent.name),
            String(args.sessionId ?? ""),
          );
          if (!conversation) return Promise.resolve(null);
          emitDemoAgentEvent(conversation, {
            type: "message_start",
            message: { role: "user", content: String(args.message ?? ""), timestamp: Date.now() },
          });
          emitDemoAgentEvent(conversation, {
            type: "message_end",
            message: { role: "user", content: String(args.message ?? ""), timestamp: Date.now() },
          });
          return Promise.resolve(null);
        }
        case "resolve_approval":
          return Promise.resolve(null);
        case "stop_run": {
          const conversation = conversationOf(
            String(args.agentName ?? demoAgent.name),
            String(args.sessionId ?? ""),
          );
          if (!conversation) return Promise.resolve(null);
          if (conversation.runTimer) clearTimeout(conversation.runTimer);
          conversation.runTimer = null;
          if (conversation.running) emitDemoAgentEvent(conversation, { type: "agent_end" });
          conversation.running = false;
          return Promise.resolve(null);
        }
        case "set_session_model":
          return Promise.resolve(null);
        // 释放该 Agent 名下**空闲**的会话（正在跑的留着）—— 与核心同构
        case "new_session": {
          const agentName = String(args.agentName ?? demoAgent.name);
          for (const [key, conversation] of [...demoConversations.entries()]) {
            if (
              conversation.agentName === agentName
              && !conversation.running
              && !conversation.temporary
            ) {
              demoConversations.delete(key);
            }
          }
          return Promise.resolve(null);
        }
        // —— 归档 / 恢复 / 删除（演示桩：内存里挪动 demo 数据）——
        case "list_archived_agents":
          return Promise.resolve(
            [demoAgent, demoPeer].filter(
              (agent) => demoAgentArchived.has(agent.name) && !demoAgentDeleted.has(agent.name),
            ),
          );
        case "archive_agent": {
          const name = String(args.name ?? "");
          const current = testConversationOf(name);
          if (current?.running) {
            return Promise.reject(new Error(`Agent「${name}」的临时测试仍在运行，请先停止再归档`));
          }
          if (current) demoConversations.delete(conversationKey(name, current.sessionId));
          if (!demoAgentDeleted.has(name)) demoAgentArchived.add(name);
          return Promise.resolve(null);
        }
        case "restore_agent": {
          const name = String(args.name ?? "");
          if (!demoAgentDeleted.has(name)) demoAgentArchived.delete(name);
          return Promise.resolve(null);
        }
        case "delete_agent": {
          const name = String(args.name ?? "");
          const current = testConversationOf(name);
          if (current?.running) {
            return Promise.reject(new Error(`Agent「${name}」的临时测试仍在运行，请先停止再删除`));
          }
          if (current) demoConversations.delete(conversationKey(name, current.sessionId));
          // 真实后端：删除目录 → Agent 永久消失（不复活，不重新播种）
          demoAgentDeleted.add(name);
          demoAgentArchived.delete(name);
          const state = demoStates.get(name);
          if (state) {
            state.histories.clear();
            state.archivedSessions.clear();
          }
          return Promise.resolve(null);
        }
        case "delete_archived_agent": {
          const name = String(args.name ?? "");
          demoAgentDeleted.add(name);
          demoAgentArchived.delete(name);
          return Promise.resolve(null);
        }
        case "list_archived_sessions": {
          const state = stateFor(String(args.agentName ?? demoAgent.name));
          return Promise.resolve(
            [...state.archivedSessions.entries()].map(([id, messages]) => ({
              id,
              title: state.titles.get(id) ?? "已归档会话",
              messageCount: messages.length,
              startedAt: Number(id.split("-")[0]) || 0,
              lastActive: Number(id.split("-")[0]) || 0,
            })),
          );
        }
        case "archive_session": {
          const state = stateFor(String(args.agentName ?? demoAgent.name));
          const sid = String(args.sessionId ?? "");
          const history = state.histories.get(sid);
          if (history !== undefined) {
            state.histories.delete(sid);
            state.archivedSessions.set(sid, history);
          }
          return Promise.resolve(null);
        }
        case "restore_session": {
          const state = stateFor(String(args.agentName ?? demoAgent.name));
          const sid = String(args.sessionId ?? "");
          const history = state.archivedSessions.get(sid);
          if (history !== undefined) {
            state.archivedSessions.delete(sid);
            state.histories.set(sid, history);
          }
          return Promise.resolve(null);
        }
        case "delete_session": {
          const state = stateFor(String(args.agentName ?? demoAgent.name));
          state.histories.delete(String(args.sessionId ?? ""));
          return Promise.resolve(null);
        }
        case "delete_archived_session": {
          const state = stateFor(String(args.agentName ?? demoAgent.name));
          state.archivedSessions.delete(String(args.sessionId ?? ""));
          return Promise.resolve(null);
        }
        default:
          return Promise.resolve(null);
      }
  };
  w.__PIPI_DEV_PLATFORM__ = {
    invoke: <T>(command: string, args?: Record<string, unknown>) => invoke(command, args) as Promise<T>,
    listen: async (event, listener) => listenDevEvent(event, listener),
  };
  w.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: (cb: unknown) => cb,
  };
  console.info("[pipi] dev mock installed（浏览器模式，无 Tauri 后端）");
}
