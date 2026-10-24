/** 后端提供类别；旧会话只使用保守的文案兜底，原始错误始终保留。 */
import { getDefaultLocale, type Locale } from "../i18n/core.ts";
import { zh } from "../i18n/zh.ts";
import { en } from "../i18n/en.ts";
import { hasVisibleText, type AssistantErrorKind } from "../chat-runtime.ts";

export type ChatErrorCategory = "aborted" | "auth" | "rate_limit" | "quota" | "context_overflow"
  | "network" | "protocol" | "permission" | "generic";
export interface ChatErrorInfo {
  category: ChatErrorCategory;
  title: string;
  badge: string;
  rawError: string;
  suggestion?: string;
}

function legacyCategory(raw: string): ChatErrorCategory {
  if (/context_length_exceeded|maximum context length|prompt is too long|too many tokens|exceeds (?:the )?context window/i.test(raw)) return "context_overflow";
  if (/insufficient_quota|quota exceeded|quota_exceeded|credit balance|账户余额已耗尽/i.test(raw)) return "quota";
  if (/rate.?limit|too many requests|(?:^|http[ :]+)429\b|限流/i.test(raw)) return "rate_limit";
  if (/invalid.?authentication|unauthorized|invalid api key|incorrect api key|invalid_api_key|(?:^|http[ :]+)(?:401|403)\b|鉴权失败/i.test(raw)) return "auth";
  if (/modelprotocolunsupported|does not support this protocol|协议已固定|协议均已尝试|协议不匹配/i.test(raw)) return "protocol";
  if (/permission denied|权限拦截|命令被拒绝|安全策略|沙箱重定向/i.test(raw)) return "permission";
  if (/timeout|timed out|connection|econnrefused|etimedout|stream ended|stream closed|ended before the provider ended it|fetch failed|network|reset by peer|流提前结束|请求超时|连接被中断|连接重置/i.test(raw)) return "network";
  return "generic";
}

export function classifyChatError(input: {
  errorMessage?: string | null;
  errorKind?: AssistantErrorKind | null;
  text?: string | null;
  status?: string | null;
  stopReason?: string | null;
}, locale: Locale = "zh"): ChatErrorInfo {
  const aborted = input.status === "aborted" || input.stopReason === "aborted";
  let raw = (input.errorMessage ?? "").trim();
  if (!raw && input.text?.trim().startsWith("⚠ ")) raw = input.text.trim().slice(2).trim();
  if (!raw) raw = aborted ? (locale === "en" ? "Aborted" : "已中止")
    : (locale === "en" ? "Agent returned an unknown error" : "Agent 返回未知错误");
  const dict = locale === "en" ? en.errors : zh.errors;
  if (aborted) return { category: "aborted", title: dict.abortedTitle, badge: dict.abortedBadge, rawError: raw };
  const categories: Record<AssistantErrorKind, ChatErrorCategory> = {
    auth: "auth", rateLimit: "rate_limit", quota: "quota", contextOverflow: "context_overflow",
    network: "network", protocol: "protocol", generic: "generic",
  };
  const category = input.errorKind && Object.hasOwn(categories, input.errorKind)
    ? categories[input.errorKind] : legacyCategory(raw);
  const presentation = {
    auth: [dict.authTitle, dict.authBadge, dict.authSuggestion],
    rate_limit: [dict.rateLimitTitle, dict.rateLimitBadge, dict.rateLimitSuggestion],
    quota: [dict.quotaTitle, dict.quotaBadge, dict.quotaSuggestion],
    context_overflow: [dict.contextTitle, dict.contextBadge, dict.contextSuggestion],
    network: [dict.networkTitle, dict.networkBadge, dict.networkSuggestion],
    protocol: [dict.protocolTitle, dict.protocolBadge, dict.protocolSuggestion],
    permission: [dict.permissionTitle, dict.permissionBadge, dict.permissionSuggestion],
    generic: [dict.genericTitle, dict.genericBadge, dict.genericSuggestion],
  };
  const [title, badge, suggestion] = presentation[category as Exclude<ChatErrorCategory, "aborted">];
  return { category, title, badge, suggestion, rawError: raw };
}

/**
 * 从可能混杂了错误/中止标记的助手文本中，拆分出干净的生成正文与错误信息对象。
 */
export function extractAssistantContentAndError(
  entry: {
    text?: string;
    errorMessage?: string;
    errorKind?: AssistantErrorKind;
    status?: string;
    stopReason?: string;
  },
  locale: Locale = getDefaultLocale(),
): {
  cleanText: string;
  errorInfo: ChatErrorInfo | null;
} {
  const isAborted = entry.status === "aborted" || entry.stopReason === "aborted";
  const isError =
    entry.status === "error" ||
    entry.stopReason === "error" ||
    Boolean(entry.errorMessage);

  const rawText = entry.text ?? "";

  // 既无报错也无中止
  if (!isAborted && !isError) {
    return { cleanText: hasVisibleText(rawText) ? rawText : "", errorInfo: null };
  }

  // 处理文本中的中止标记
  let cleanText = rawText;
  if (cleanText.endsWith("\n■ 已中止")) {
    cleanText = cleanText.slice(0, -"\n■ 已中止".length);
  } else if (cleanText === "■ 已中止") {
    cleanText = "";
  }

  // 处理纯错误占位符
  if (cleanText === `⚠ ${entry.errorMessage}` || cleanText === "⚠ Agent 返回错误"
    || (!entry.errorMessage && cleanText.startsWith("⚠ "))) {
    // 纯错误消息不作为正文
    cleanText = "";
  }

  const errorInfo = classifyChatError(
    {
      errorMessage: entry.errorMessage,
      errorKind: entry.errorKind,
      text: rawText,
      status: entry.status,
      stopReason: entry.stopReason,
    },
    locale,
  );

  return { cleanText: hasVisibleText(cleanText) ? cleanText : "", errorInfo };
}
