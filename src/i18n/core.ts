import { zh, type I18nSchema } from "./zh.ts";
import { en } from "./en.ts";

export type Locale = "zh" | "en";

export const LOCALES: { id: Locale; label: string }[] = [
  { id: "zh", label: "简体中文" },
  { id: "en", label: "English" },
];

export const dictionaries: Record<Locale, I18nSchema> = { zh, en };

export const STORAGE_LOCALE_KEY = "pipi-locale";

/** 检测默认语言（首选 localStorage，其次操作系统/浏览器环境）。 */
export function getDefaultLocale(): Locale {
  try {
    const saved = typeof localStorage !== "undefined" ? localStorage.getItem(STORAGE_LOCALE_KEY) : null;
    if (saved === "zh" || saved === "en") return saved;
  } catch {
    // 忽略 localStorage 读取异常
  }
  if (typeof navigator !== "undefined" && typeof navigator.language === "string") {
    if (navigator.language.toLowerCase().startsWith("zh")) return "zh";
  }
  return "en";
}

/**
 * 纯函数翻译提取（可在 React 组件外使用）。
 *
 * @param locale 语言代码
 * @param path 路径点分字符串，例如 "common.confirm"
 * @param params 可选插值变量映射，例如 `{ count: 3 }`
 */
export function getTranslation(
  locale: Locale,
  path: string,
  params?: Record<string, string | number>,
): string {
  const keys = path.split(".");
  let cur: unknown = dictionaries[locale] ?? dictionaries.zh;

  for (const key of keys) {
    if (cur && typeof cur === "object" && key in cur) {
      cur = (cur as Record<string, unknown>)[key];
    } else {
      cur = undefined;
      break;
    }
  }

  // 缺失时回退到中文
  if (typeof cur !== "string" && locale !== "zh") {
    let fallback: unknown = dictionaries.zh;
    for (const key of keys) {
      if (fallback && typeof fallback === "object" && key in fallback) {
        fallback = (fallback as Record<string, unknown>)[key];
      } else {
        fallback = undefined;
        break;
      }
    }
    if (typeof fallback === "string") cur = fallback;
  }

  let result = typeof cur === "string" ? cur : path;

  if (params) {
    result = result.replace(/\{(\w+)\}/g, (_, k: string) =>
      params[k] != null ? String(params[k]) : `{${k}}`,
    );
  }

  return result;
}

export { zh, en, type I18nSchema };
