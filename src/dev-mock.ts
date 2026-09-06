// 仅开发环境生效：在普通浏览器（无 Tauri 后端）里提供最小 invoke 桩，
// 让前端 UI 可以脱离桌面壳独立开发调试。Tauri 生产构建不受影响。
import type { Settings } from "./App";

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
          return Promise.resolve([]);
        default:
          return Promise.resolve(null);
      }
    },
    transformCallback: (cb) => cb,
  };
  console.info("[pipi] dev mock installed（浏览器模式，无 Tauri 后端）");
}
