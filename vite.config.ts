import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // 监听所有网卡：手机等局域网设备可用 http://<本机IP>:1420 访问（仅开发模式生效）
    host: true,
    // 忽略 Rust 构建产物、桌面壳与上游源码快照：数量庞大且无需 HMR
    //（target/ 与 reference/ 都会耗尽 inotify watch → ENOSPC 直接打挂 dev server）
    watch: process.env.PIPI_DESKTOP_NO_WATCH === "1"
      ? null
      : { ignored: ["**/crates/pipi/**", "**/target/**", "**/reference/**"] },
    hmr: process.env.PIPI_DESKTOP_NO_WATCH === "1" ? false : undefined,
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  // reference/ 内的上游 HTML 不是应用入口，避免扫描它们的独立依赖树。
  optimizeDeps: { entries: ["index.html"] },
  build: {
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
  },
});
