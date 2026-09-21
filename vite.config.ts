import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 版本号注入前端（侧栏与状态栏展示），单一来源：package.json
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf-8"),
) as { version: string };

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  define: {
    __PIPI_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 1420,
    strictPort: true,
    // 监听所有网卡：手机等局域网设备可用 http://<本机IP>:1420 访问（仅开发模式生效）
    host: true,
    // 忽略 Rust 构建产物、桌面壳与上游源码快照：数量庞大且无需 HMR
    //（target/ 与 reference/ 都会耗尽 inotify watch → ENOSPC 直接打挂 dev server）
    watch: { ignored: ["**/src-tauri/**", "**/target/**", "**/reference/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
  },
});
