import type { ThemeDefinition } from "../types.ts";

export const lightTheme: ThemeDefinition = {
  id: "light",
  label: "极简浅色",
  description: "纯白内容区与柔和灰阶，让阅读和对话更专注。",
  mode: "light",
  preview: {
    bg: "#ffffff",
    nav: "#f9f9f9",
    rail: "#eeeeee",
    card: "#ffffff",
    line: "#dedede",
    accent: "#303030",
  },
  tokens: {}, // 全量使用 BASE_LIGHT_TOKENS
};
