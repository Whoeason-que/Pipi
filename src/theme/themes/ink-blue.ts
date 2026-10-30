import type { ThemeDefinition } from "../types.ts";

export const inkBlueTheme: ThemeDefinition = {
  id: "ink-blue",
  label: "墨蓝工作台",
  description: "Pipi 默认暗色工作台，沉稳墨蓝底色与柔和冷蓝强调。",
  mode: "dark",
  preview: {
    bg: "#10151e",
    nav: "#0f141c",
    rail: "#0a0e15",
    card: "#1a212d",
    line: "#2f3949",
    accent: "#8fa9ff",
  },
  tokens: {}, // 全量使用 BASE_DARK_TOKENS
};
