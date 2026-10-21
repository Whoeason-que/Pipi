// npm run dev 后打开 /tests/files.browser.html。仅使用假数据，不连接真实 Agent 或发送模型请求。
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { I18nProvider } from "../src/i18n";
import { installDevMock } from "../src/dev-mock";
import type { WorkspaceFile } from "../src/files/types";
import "../src/styles.css";

const contents: Record<string, string> = {
  "long.ts": Array.from({ length: 500 }, (_, i) => `export const row${i + 1} = ${i + 1}; // row ${i + 1}`).join("\n"),
  "staged.ts": "export const staged = true;",
  "new.txt": "new file",
  "hello.ts": "// 文件预览\nexport const greeting = '你好';\nconsole.log(greeting);\n",
  "readme.md": "# 文件预览\n\n**Markdown** 渲染及 [本地源码](hello.ts)。\n\n![外部图片](https://example.invalid/no-network.png)\n\n<script>document.title='UNSAFE'</script>",
  "src/slow.ts": "export const file = 'slow';",
  "src/fast.ts": "export const file = 'fast';",
};
installDevMock();
const platform = window.__PIPI_DEV_PLATFORM__!;
const originalFetch = window.fetch;
window.fetch = (input, init) => String(input).includes("/api/auth/status")
  ? Promise.resolve(new Response(JSON.stringify({ authRequired: false, authenticated: true }), { status: 200 })) : originalFetch(input, init);
window.__PIPI_DEV_PLATFORM__ = {
  ...platform,
  invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
    const path = String(args?.path ?? "");
    const workspace = `/fixture/${args?.agentName ?? "unknown"}`;
    let result: unknown;
    if (command === "list_workspace_files") result = { workspace, path, truncated: false, entries: path === "src" ? ["src/slow.ts", "src/fast.ts"].map(path => ({ name: path.split("/").pop(), path, kind: "file", size: 40 })) : [
      { name: "src", path: "src", kind: "directory", size: 0 },
      { name: "hello.ts", path: "hello.ts", kind: "file", size: 90 },
      { name: "readme.md", path: "readme.md", kind: "file", size: 120 },
      { name: "image.png", path: "image.png", kind: "file", size: 100 },
      { name: "escape", path: "escape", kind: "symlink", size: 0 },
    ] };
    else if (command === "preview_workspace_file") {
      if (path === "src/slow.ts") await new Promise(resolve => setTimeout(resolve, 600));
      if (!(path in contents) && path !== "image.png") throw new Error("文件不存在或不在工作目录内");
      result = { workspace, path, size: 100, version: `fixture-${path}`, kind: path.endsWith("md") ? "markdown" : path.endsWith("png") ? "image" : "text", mime: path.endsWith("png") ? "image/png" : "text/plain", content: contents[path] ?? null, base64: path.endsWith("png") ? "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jWZkAAAAASUVORK5CYII=" : null, truncated: false } satisfies WorkspaceFile;
    } else if (command === "workspace_changes") result = { workspace, isRepo: args?.agentName !== "demo-reviewer", files: args?.agentName === "demo-reviewer" ? [] : [{ path: "staged.ts", status: "M " }, { path: "hello.ts", status: " M" }, { path: "deleted.txt", status: " D" }, { path: "new.txt", status: "??" }] };
    else if (command === "workspace_diff") result = { path, base: "HEAD", patch: "--- a/hello.ts\n+++ b/hello.ts\n@@ -1 +1 @@\n-before\n+after\n", truncated: false };
    else if (command === "open_session") {
      const snapshot = await platform.invoke<{ info: unknown; messages: unknown[]; stats: unknown }>(command, args);
      result = { ...snapshot, messages: [...snapshot.messages,
        { role: "assistant", content: [{ type: "toolCall", id: "fixture-edit", name: "edit", arguments: { path: "hello.ts", oldText: "before", newText: "after" } }], timestamp: 42 },
        { role: "toolResult", toolCallId: "fixture-edit", toolName: "edit", content: [{ type: "text", text: "edited" }], details: { diff: "-before\n+after" }, isError: false, timestamp: 43 },
      ] };
    } else return platform.invoke<T>(command, args);
    return result as T;
  },
  listen: async () => () => {},
};

const root = createRoot(document.getElementById("root")!);
root.render(<StrictMode><I18nProvider><App /></I18nProvider></StrictMode>);
import.meta.hot?.dispose(() => { root.unmount(); window.fetch = originalFetch; });
