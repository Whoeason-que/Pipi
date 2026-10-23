// npm run dev 后打开 /tests/chat-errors.browser.html；?mode=uncertain 验证发送响应丢失。
// 全部为本地假数据，不连接真实宿主或模型。
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { I18nProvider } from "../src/i18n";
import { installDevMock } from "../src/dev-mock";
import { CommandTransportError } from "../src/transport-error";
import "../src/styles.css";

installDevMock();
const platform = window.__PIPI_DEV_PLATFORM__!;
const originalFetch = window.fetch;
const uncertain = new URLSearchParams(location.search).get("mode") === "uncertain";
let offline = false;
window.fetch = (input, init) => String(input).includes("/api/auth/status")
  ? Promise.resolve(new Response(JSON.stringify({ authRequired: false, authenticated: true }), { status: 200 }))
  : originalFetch(input, init);
window.__PIPI_DEV_PLATFORM__ = {
  ...platform,
  invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
    if (uncertain && command === "send_prompt") {
      offline = true;
      throw new CommandTransportError("Simulated lost send response");
    }
    if (offline && command === "session_info") throw new CommandTransportError("Simulated offline host");
    if (command === "stop_run") offline = false;
    if (!uncertain && command === "open_session") {
      const snapshot = await platform.invoke<{ info: unknown; messages: unknown[]; stats: unknown }>(command, args);
      return { ...snapshot, messages: [...snapshot.messages,
        { role: "assistant", content: "这部分回答已生成，连接中断后仍然保留。", stopReason: "error",
          errorMessage: "429: insufficient_quota — account balance exhausted", errorKind: "quota", timestamp: 100 },
      ] } as T;
    }
    return platform.invoke<T>(command, args);
  },
};
const root = createRoot(document.getElementById("root")!);
root.render(<StrictMode><I18nProvider><App /></I18nProvider></StrictMode>);
import.meta.hot?.dispose(() => { root.unmount(); window.fetch = originalFetch; });
