import { compileThemeTokens, resolveTheme, DEFAULT_THEME_ID } from "./registry.ts";
import type { ThemeDefinition } from "./types.ts";

export const STORAGE_THEME_KEY = "pipi-theme";

/**
 * 将驼峰属性名转换为 CSS 变量命名（支持数字后缀）：
 * 例：panel2 -> --panel-2, navBg -> --nav-bg, avatarBlueFg -> --avatar-blue-fg
 */
export function toCssVarName(prop: string): string {
  return "--" + prop.replace(/([A-Z])|(\d+)/g, "-$1$2").toLowerCase();
}

/**
 * 将指定主题应用至页面：
 * 1. 设置 html[data-theme="dark"|"light"]（维持既有明暗选择器与原生滚动条色彩）
 * 2. 设置 html[data-palette="theme-id"]
 * 3. 动态注入 <style id="pipi-theme-tokens"> 变量样式块，毫秒级即时生效
 * 4. 写入 localStorage 保证刷新不闪烁
 */
export function applyTheme(themeId: string | null | undefined): ThemeDefinition {
  const theme = resolveTheme(themeId);
  const { mode, tokens } = compileThemeTokens(theme);

  if (typeof document !== "undefined") {
    const root = document.documentElement;

    root.dataset.theme = mode;
    root.dataset.palette = theme.id;
    root.style.colorScheme = mode;

    let styleTag = document.getElementById("pipi-theme-tokens") as HTMLStyleElement | null;
    if (!styleTag) {
      styleTag = document.createElement("style");
      styleTag.id = "pipi-theme-tokens";
      document.head.appendChild(styleTag);
    }

    const lines: string[] = [];
    for (const [key, value] of Object.entries(tokens)) {
      if (typeof value === "string" && value.length > 0) {
        lines.push(`  ${toCssVarName(key)}: ${value};`);
      }
    }

    // 高于 tokens.css 的 :root[data-theme]，样式在启动或 HMR 后重排也不丢失配色。
    styleTag.textContent = `:root[data-theme][data-palette] {\n${lines.join("\n")}\n}`;
  }

  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(STORAGE_THEME_KEY, theme.id);
    }
  } catch {
    // 忽略受限环境下的 localStorage 读写异常
  }

  return theme;
}

/**
 * 读取初始主题 ID（首选 localStorage，其次回退到默认主题）
 */
export function getInitialThemeId(): string {
  try {
    if (typeof localStorage !== "undefined") {
      const saved = localStorage.getItem(STORAGE_THEME_KEY);
      if (saved) return saved;
    }
  } catch {
    // 忽略异常
  }
  return DEFAULT_THEME_ID;
}

export * from "./types.ts";
export * from "./base.ts";
export * from "./registry.ts";
