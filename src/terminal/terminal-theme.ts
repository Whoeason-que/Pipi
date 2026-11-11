import type { ITheme } from "@xterm/xterm";

/** ANSI palettes are fixed; chrome colors come from the app's theme tokens. */
const DARK_ANSI: ITheme = {
  black: "#15161e",
  red: "#f7768e",
  green: "#9ece6a",
  yellow: "#e0af68",
  blue: "#7aa2f7",
  magenta: "#bb9af7",
  cyan: "#7dcfff",
  white: "#c0caf5",
  brightBlack: "#414868",
  brightRed: "#ff899d",
  brightGreen: "#b9f27c",
  brightYellow: "#ffcf7a",
  brightBlue: "#9ab8ff",
  brightMagenta: "#d6b4ff",
  brightCyan: "#a4eaff",
  brightWhite: "#ffffff",
};

const LIGHT_ANSI: ITheme = {
  black: "#1f2335",
  red: "#c53b53",
  green: "#3d7a2f",
  yellow: "#8a5a00",
  blue: "#2f5fb3",
  magenta: "#7a3ea8",
  cyan: "#0f6f86",
  white: "#e6e7ed",
  brightBlack: "#5a5f73",
  brightRed: "#a92b43",
  brightGreen: "#2f6b24",
  brightYellow: "#7a4f00",
  brightBlue: "#244f9c",
  brightMagenta: "#673292",
  brightCyan: "#0b5d72",
  brightWhite: "#ffffff",
};

export function terminalTheme(): ITheme {
  if (typeof document === "undefined") return { ...DARK_ANSI };
  const styles = getComputedStyle(document.documentElement);
  const value = (name: string, fallback: string) =>
    styles.getPropertyValue(name).trim() || fallback;
  const dark = document.documentElement.dataset.theme !== "light";
  const palette = dark ? DARK_ANSI : LIGHT_ANSI;
  return {
    ...palette,
    background: value("--card", dark ? "#1a1b26" : "#ffffff"),
    foreground: value("--fg", dark ? "#c0caf5" : "#1f2335"),
    cursor: value("--accent", dark ? "#7aa2f7" : "#34548a"),
    cursorAccent: value("--card", dark ? "#1a1b26" : "#ffffff"),
    selectionBackground: value("--accent-soft", dark ? "#33467c" : "#c7d2fe"),
  };
}
