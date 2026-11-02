import { getTranslation } from "./i18n/core.ts";
import type { ReasoningEffort } from "./types.ts";

/**
 * 思考档位选项。`en` 只选字典：默认英文（权威字典），zh 缺键时回退英文；
 * value 是协议值，任何语言下都不变。
 */
export function reasoningChoices(en = true): { value: ReasoningEffort | "default"; label: string }[] {
  const locale = en ? "en" : "zh";
  return [
    { value: "default", label: getTranslation(locale, "app.reasoning.default") },
    { value: "none", label: getTranslation(locale, "app.reasoning.none") },
    { value: "minimal", label: getTranslation(locale, "app.reasoning.minimal") },
    { value: "low", label: getTranslation(locale, "app.reasoning.low") },
    { value: "medium", label: getTranslation(locale, "app.reasoning.medium") },
    { value: "high", label: getTranslation(locale, "app.reasoning.high") },
    { value: "xhigh", label: getTranslation(locale, "app.reasoning.xhigh") },
    { value: "max", label: getTranslation(locale, "app.reasoning.max") },
  ];
}
