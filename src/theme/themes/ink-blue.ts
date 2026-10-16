import type { ThemeDefinition } from "../types.ts";

export const inkBlueTheme: ThemeDefinition = {
  id: "ink-blue",
  label: "墨蓝工作台",
  description: "Pipi 默认暗色工作台，沉稳墨蓝底色与柔和冷蓝强调。",
  mode: "dark",
  preview: {
    bg: "#141a24",
    nav: "#101620",
    rail: "#0c121b",
    card: "#1d2634",
    line: "#334057",
    accent: "#91acff",
  },
  tokens: {}, // 全量使用 BASE_DARK_TOKENS
};
