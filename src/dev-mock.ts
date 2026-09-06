// 仅开发环境生效：在普通浏览器（无 Tauri 后端）里提供最小 invoke 桩，
// 让前端 UI 可以脱离桌面壳独立开发调试。Tauri 生产构建不受影响。
import type { AgentDefinition, Settings } from "./types";

const demoAgent: AgentDefinition = {
  name: "demo-assistant",
  description: "浏览器演示模式的示例 Agent（真实 Agent 由桌面端读写 ~/.pipi/agents）",
  model: "",
  provider: null,
  workspace: null,
  permissions: {
    tools: ["read", "write", "edit", "bash", "memory"],
    bash: { mode: "allowAll", commands: [] },
    sandbox: "workspace-write",
  },
  mcpServers: [],
};

const demoMessages = [
  { role: "user", content: "帮我看看这个项目的结构", timestamp: 0 },
  {
    role: "assistant",
    content: [{ type: "text", text: "我先用 bash 看一下目录。" }],
    usage: { input: 320, output: 48, cacheRead: 280, cacheWrite: 0, totalTokens: 648 },
    stopReason: "toolUse",
    timestamp: 0,
    durationMs: 1900,
  },
  { role: "toolResult", toolCallId: "t1", toolName: "bash", content: [{ type: "toolResultText", text: "src/ README.md" }], isError: false, timestamp: 0 },
  {
    role: "assistant",
    content: [{ type: "text", text: "项目结构很简洁：src/ 放前端，src-tauri/ 放 Rust 核心，crates/pipi-core 是从 pi 移植的 Agent 内核。" }],
    usage: { input: 410, output: 96, cacheRead: 380, cacheWrite: 0, totalTokens: 886 },
    stopReason: "stop",
    timestamp: 0,
    durationMs: 2600,
  },
];

interface TauriInternals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  transformCallback: (cb: unknown) => unknown;
}

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

export function installDevMock(): void {
  if (!import.meta.env.DEV) return;
  const w = window as unknown as { __TAURI_INTERNALS__?: TauriInternals };
  if (w.__TAURI_INTERNALS__) return;
  w.__TAURI_INTERNALS__ = {
    invoke: (cmd, args = {}) => {
      switch (cmd) {
        case "get_settings":
          return Promise.resolve(structuredClone(settings));
        case "save_settings":
          Object.assign(settings, args.settings);
          return Promise.resolve(null);
        case "list_agents":
          return Promise.resolve([demoAgent]);
        case "load_agent":
          return Promise.resolve(demoAgent);
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
        case "session_messages":
          return Promise.resolve([]);
        case "session_stats":
          return Promise.resolve({
            input: 0, output: 0, cacheRead: 0, cacheWrite: 0, calls: 0,
          });
        case "session_running":
          return Promise.resolve(false);
        case "send_prompt":
          return Promise.reject("浏览器演示模式不支持发送（需要桌面端的 Tauri 后端）");
        case "stop_run":
        case "new_session":
          return Promise.resolve(null);
        default:
          return Promise.resolve(null);
      }
    },
    transformCallback: (cb) => cb,
  };
  console.info("[pipi] dev mock installed（浏览器模式，无 Tauri 后端）");
}
