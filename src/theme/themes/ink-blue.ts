import type { ThemeDefinition } from "../types.ts";

export const inkBlueTheme: ThemeDefinition = {
  id: "ink-blue",
  label: "Ink Blue",
  description: "Pipi's default dark workspace with deep ink-blue tones and a soft cool accent.",
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
