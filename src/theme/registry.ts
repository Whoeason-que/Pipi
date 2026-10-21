import { BASE_DARK_TOKENS, BASE_LIGHT_TOKENS } from "./base.ts";
import { inkBlueTheme } from "./themes/ink-blue.ts";
import { lightTheme } from "./themes/light.ts";
import { nordTheme } from "./themes/nord.ts";
import { catppuccinTheme } from "./themes/catppuccin.ts";
import { charcoalTheme } from "./themes/charcoal.ts";
import { solarizedLightTheme } from "./themes/solarized-light.ts";
import type { ThemeDefinition, ThemeMode, ThemeTokens } from "./types.ts";

export const BUILTIN_THEMES: ThemeDefinition[] = [
  inkBlueTheme,
  lightTheme,
  nordTheme,
  catppuccinTheme,
  charcoalTheme,
  solarizedLightTheme,
];

export const DEFAULT_THEME_ID = "ink-blue";

/**
 * 解析主题标识符，内置向下兼容逻辑：
 * 历史的 "dark" 映射到默认墨蓝，"light" 映射到极简浅色。
 */
export function resolveTheme(themeId: string | null | undefined): ThemeDefinition {
  if (!themeId) return inkBlueTheme;
  if (themeId === "dark") return inkBlueTheme;
  if (themeId === "light") return lightTheme;

  const found = BUILTIN_THEMES.find((item) => item.id === themeId);
  return found ?? inkBlueTheme;
}

/**
 * 将主题与其基础模式（Dark 或 Light）基底合并，生成完整无缺失的令牌集合。
 */
export function compileThemeTokens(theme: ThemeDefinition): {
  mode: ThemeMode;
  tokens: ThemeTokens;
} {
  const base = theme.mode === "dark" ? BASE_DARK_TOKENS : BASE_LIGHT_TOKENS;
  return {
    mode: theme.mode,
    tokens: { ...base, ...theme.tokens },
  };
}
