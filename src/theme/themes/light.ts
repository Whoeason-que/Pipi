import type { ThemeDefinition } from "../types.ts";

export const lightTheme: ThemeDefinition = {
  id: "light",
  label: "极简浅色",
  description: "纯白内容区与柔和灰阶，让阅读和对话更专注。",
  mode: "light",
  preview: {
    bg: "#fbfbfc",
    nav: "#f7f7f8",
    rail: "#f0f0f2",
    card: "#ffffff",
    line: "#e2e2e5",
    accent: "#24262b",
  },
  tokens: {}, // 全量使用 BASE_LIGHT_TOKENS
};
