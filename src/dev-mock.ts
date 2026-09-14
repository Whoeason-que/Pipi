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
};

const initialDemoMessages: Array<Record<string, unknown>> = [
  { role: "user", content: "帮我看看这个项目的结构", timestamp: Date.now() - 260000 },
  {
    role: "assistant",
    content: [{ type: "text", text: "我先用 bash 看一下目录。" }],
    usage: { input: 320, output: 48, cacheRead: 280, cacheWrite: 0, totalTokens: 648 },
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
    usage: { input: 410, output: 96, cacheRead: 380, cacheWrite: 0, totalTokens: 886 },
    stopReason: "stop",
    timestamp: Date.now() - 230000,
    durationMs: 2600,
  },
];

const initialDemoSessionId = "1730000000000-abc123";
let demoSessionId = initialDemoSessionId;
let demoMessages: Array<Record<string, unknown>> = initialDemoMessages;
const demoSessionHistories = new Map<string, Array<Record<string, unknown>>>([
  [initialDemoSessionId, demoMessages],
]);
const legacyDemoSessionId = "1729990000000-def456";
demoSessionHistories.set(legacyDemoSessionId, [
  { role: "user", content: "把 README 翻译成英文", timestamp: 0 },
  { role: "assistant", content: [{ type: "text", text: "好的，我会先读取 README。" }], timestamp: 0 },
]);
const demoSessionTitles = new Map<string, string>([
  [initialDemoSessionId, "帮我看看这个项目的结构"],
  [legacyDemoSessionId, "把 README 翻译成英文"],
]);

interface DevEvent {
  payload: unknown;
}

type DevEventListener = (event: DevEvent) => void;

const devListeners = new Map<string, Set<DevEventListener>>();
let demoRunTimer: ReturnType<typeof setTimeout> | null = null;
let demoRunning = false;
let demoHasSession = true;
let demoRunId = 0;
let demoSessionCounter = 0;
// 归档演示状态：Agent 与部分会话可被「归档/恢复/删除」，在内存里挪动。
// deleted 与 archived 分开：删除是永久消失（对应真实后端删目录），
// 归档后仍可恢复（对应真实后端从 .archive 移回）。
let demoAgentArchived = false;
let demoAgentDeleted = false;
const demoArchivedSessions = new Map<string, Array<Record<string, unknown>>>();

function emitDevEvent(name: string, payload: unknown): void {
  devListeners.get(name)?.forEach((listener) => listener({ payload }));
}

function emitDemoAgentEvent(event: unknown): void {
  emitDevEvent("agent-event", {
    agentName: "demo-assistant",
    sessionId: demoSessionId,
    runId: demoRunId,
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

function startDemoRun(prompt: string): void {
  if (demoRunTimer) clearTimeout(demoRunTimer);
  demoRunId += 1;
  demoRunning = true;
  demoHasSession = true;
  demoSessionTitles.set(demoSessionId, demoSessionTitles.get(demoSessionId) ?? prompt);
  demoMessages.push({ role: "user", content: prompt, timestamp: Date.now() });
  const response = {
    role: "assistant",
    content: [{ type: "text", text: `我已收到：${prompt}\n\n这是浏览器演示模式的流式响应。` }],
    usage: { input: 120, output: 32, cacheRead: 80, cacheWrite: 0, totalTokens: 232 },
    stopReason: "stop",
    timestamp: Date.now(),
    durationMs: 320,
  };
  demoRunTimer = setTimeout(() => {
    emitDemoAgentEvent({ type: "agent_start" });
    emitDemoAgentEvent({ type: "message_start", message: { role: "assistant", content: [] } });
    emitDemoAgentEvent({
      type: "message_update",
      message: { ...response, content: [{ type: "text", text: "我已收到：" }] },
    });
    emitDemoAgentEvent({ type: "message_update", message: response });
    emitDemoAgentEvent({ type: "message_end", message: response });
    demoMessages.push(response);
    emitDevEvent("session-stats", {
      agentName: "demo-assistant",
      sessionId: demoSessionId,
      runId: demoRunId,
      stats: {
        input: 850,
        output: 176,
        cacheRead: 740,
        cacheWrite: 0,
        calls: 3,
        avgTps: 51.2,
        avgLatencyS: 1.8,
        cacheHitPct: 87.1,
        contextUsed: 990,
        contextMax: 200000,
        contextPercent: 0,
      },
    });
    emitDemoAgentEvent({ type: "agent_end", messages: [response] });
    demoRunTimer = null;
    demoRunning = false;
  }, 120);
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
};

function listDemoSessions(): Array<Record<string, unknown>> {
  return [...demoSessionHistories.entries()]
    .filter(([, messages]) => messages.length > 0)
    .map(([id, messages]) => ({
      id,
      title: demoSessionTitles.get(id) ?? "未命名会话",
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
          return Promise.resolve(demoAgentArchived || demoAgentDeleted ? [] : [demoAgent]);
        case "save_agent": {
          // 浏览器演示模式：把保存落回 demoAgent，让「改完刷新」的流程可验证
          const def = args.def as AgentDefinition | undefined;
          if (def && def.name === demoAgent.name) Object.assign(demoAgent, def);
          return Promise.resolve(null);
        }
        case "model_catalog":
          // 与 pipi-core 的 wire 结构一致（providers/models 按 camelCase）
          return Promise.resolve(structuredClone(demoCatalog));
        case "load_agent":
          return Promise.resolve(demoAgent);
        case "list_agent_files":
          return Promise.resolve(["AGENTS.md", "memory/user-prefs.md"]);
        case "read_agent_file":
          return Promise.resolve(
            String(args.relPath) === "AGENTS.md"
              ? "# demo-assistant\n\n演示模式的系统指令。\n"
              : "",
          );
        case "write_agent_file":
          return Promise.resolve(null);
        case "list_sessions":
          return Promise.resolve(listDemoSessions());
        case "open_session":
          demoSessionId = String(args.sessionId ?? demoSessionId);
          demoRunId = 0;
          demoMessages = demoSessionHistories.get(demoSessionId) ?? [];
          demoSessionHistories.set(demoSessionId, demoMessages);
          demoHasSession = demoMessages.length > 0;
          return Promise.resolve(null);
        case "session_info":
          return Promise.resolve(demoHasSession ? {
            agentName: "demo-assistant",
            sessionId: demoSessionId,
            running: demoRunning,
            runId: demoRunId,
          } : null);
        case "session_messages":
          return Promise.resolve(demoMessages);
        case "session_stats":
          return Promise.resolve({
            input: 730,
            output: 144,
            cacheRead: 660,
            cacheWrite: 0,
            calls: 2,
            avgTps: 53.8,
            avgLatencyS: 2.25,
            cacheHitPct: 62.9,
            contextUsed: 814,
            contextMax: 200000,
            contextPercent: 0,
          });
        case "session_running":
          return Promise.resolve(demoRunning);
        case "send_prompt":
          startDemoRun(String(args.prompt ?? ""));
          return Promise.resolve(null);
        case "stop_run":
          if (demoRunTimer) clearTimeout(demoRunTimer);
          demoRunTimer = null;
          if (demoRunning) emitDemoAgentEvent({ type: "agent_end" });
          demoRunning = false;
          return Promise.resolve(null);
        case "new_session":
          if (demoRunTimer) clearTimeout(demoRunTimer);
          demoRunTimer = null;
          demoRunning = false;
          demoHasSession = false;
          demoSessionCounter += 1;
          demoSessionId = `demo-session-${demoSessionCounter}`;
          demoRunId = 0;
          demoMessages = [];
          demoSessionHistories.set(demoSessionId, demoMessages);
          return Promise.resolve(null);
        // —— 归档 / 恢复 / 删除（演示桩：内存里挪动 demo 数据）——
        case "list_archived_agents":
          return Promise.resolve(demoAgentArchived && !demoAgentDeleted ? [demoAgent] : []);
        case "archive_agent":
          if (String(args.name ?? "") === demoAgent.name && !demoAgentDeleted) {
            demoAgentArchived = true;
          }
          return Promise.resolve(null);
        case "restore_agent":
          if (String(args.name ?? "") === demoAgent.name && !demoAgentDeleted) {
            demoAgentArchived = false;
          }
          return Promise.resolve(null);
        case "delete_agent":
          if (String(args.name ?? "") === demoAgent.name) {
            // 真实后端：删除目录 → Agent 永久消失（不复活，不重新播种）
            demoAgentDeleted = true;
            demoAgentArchived = false;
            demoSessionHistories.clear();
            demoArchivedSessions.clear();
          }
          return Promise.resolve(null);
        case "delete_archived_agent":
          demoAgentDeleted = true;
          demoAgentArchived = false;
          return Promise.resolve(null);
        case "list_archived_sessions":
          return Promise.resolve(
            [...demoArchivedSessions.entries()].map(([id, messages]) => ({
              id,
              title: demoSessionTitles.get(id) ?? "已归档会话",
              messageCount: messages.length,
              startedAt: Number(id.split("-")[0]) || 0,
              lastActive: Number(id.split("-")[0]) || 0,
            })),
          );
        case "archive_session": {
          const sid = String(args.sessionId ?? "");
          const history = demoSessionHistories.get(sid);
          if (history !== undefined) {
            demoSessionHistories.delete(sid);
            demoArchivedSessions.set(sid, history);
          }
          return Promise.resolve(null);
        }
        case "restore_session": {
          const sid = String(args.sessionId ?? "");
          const history = demoArchivedSessions.get(sid);
          if (history !== undefined) {
            demoArchivedSessions.delete(sid);
            demoSessionHistories.set(sid, history);
          }
          return Promise.resolve(null);
        }
        case "delete_session": {
          const sid = String(args.sessionId ?? "");
          demoSessionHistories.delete(sid);
          return Promise.resolve(null);
        }
        case "delete_archived_session": {
          const sid = String(args.sessionId ?? "");
          demoArchivedSessions.delete(sid);
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
