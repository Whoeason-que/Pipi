import type { Theme } from "./types";
export function applyTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("pipi-theme", theme);
}
