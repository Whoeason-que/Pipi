import type { ThemeDefinition } from "../types.ts";

export const lightTheme: ThemeDefinition = {
  id: "light",
  label: "Minimal Light",
  description: "Pure white content areas with soft grays for focused reading and conversation.",
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
