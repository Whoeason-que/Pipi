import { zh } from "./zh.ts";
import { en, type DeepPartial, type I18nSchema } from "./en.ts";
import { readStored } from "../storage.ts";

export type Locale = "zh" | "en";

export const dictionaries: Record<Locale, DeepPartial<I18nSchema>> = { en, zh };

export const STORAGE_LOCALE_KEY = "pipi-locale";

/** 默认语言是 en；只有用户显式选过 zh 才用中文（zh 未覆盖的键回退到 en）。 */
export function getDefaultLocale(): Locale {
  const saved = readStored(STORAGE_LOCALE_KEY, "");
  if (saved === "zh" || saved === "en") return saved;
  return "en";
}

function lookup(dict: unknown, keys: string[]): unknown {
  let cur: unknown = dict;
  for (const key of keys) {
    if (cur && typeof cur === "object" && key in cur) {
      cur = (cur as Record<string, unknown>)[key];
    } else {
      return undefined;
    }
  }
  return cur;
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

  // 当前语言缺失时回退到 en（权威字典）
  let cur = lookup(dictionaries[locale] ?? en, keys);
  if (typeof cur !== "string") cur = lookup(en, keys);

  let result = typeof cur === "string" ? cur : path;

  if (params) {
    result = result.replace(/\{(\w+)\}/g, (_, k: string) =>
      params[k] != null ? String(params[k]) : `{${k}}`,
    );
  }

  return result;
}

export { zh, en, type I18nSchema };
