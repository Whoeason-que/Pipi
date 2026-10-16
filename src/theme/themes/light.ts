import type { ThemeDefinition } from "../types.ts";

export const lightTheme: ThemeDefinition = {
  id: "light",
  label: "浅蓝日间",
  description: "清爽透亮的明亮主题，适合日间光照环境使用。",
  mode: "light",
  preview: {
    bg: "#f8faff",
    nav: "#f0f4fa",
    rail: "#e9eef8",
    card: "#ffffff",
    line: "#cbd6e8",
    accent: "#365edc",
  },
  tokens: {}, // 全量使用 BASE_LIGHT_TOKENS
};
