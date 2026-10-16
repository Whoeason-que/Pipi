/**
 * 会话错误分类与辅助解析。
 *
 * 针对模型调用、网络连接、上下文限制、认证授权及用户中止等各类错误，
 * 提供语义化分类、差异化展示标签、友好的操作建议与详情提取。
 */

export type ChatErrorCategory =
  | "aborted"
  | "auth"
  | "rate_limit"
  | "context_overflow"
  | "network"
  | "protocol"
  | "permission"
  | "generic";

export interface ChatErrorInfo {
  category: ChatErrorCategory;
  /** 人类友好的标题，例如 "API 认证失败"、"请求频率超限"、"网络连接中断" */
  title: string;
  /** 简短分类徽章文本，例如 "认证异常"、"频率限制"、"已中止" */
  badge: string;
  /** 原始错误消息正文（去除 ⚠ 等前缀） */
  rawError: string;
  /** 面向用户的指导性操作建议 */
  suggestion?: string;
}

export function classifyChatError(input: {
  errorMessage?: string | null;
  text?: string | null;
  status?: string | null;
  stopReason?: string | null;
}): ChatErrorInfo {
  const status = input.status ?? undefined;
  const stopReason = input.stopReason ?? undefined;

  let raw = (input.errorMessage ?? "").trim();
  if (!raw && input.text) {
    const trimmed = input.text.trim();
    if (trimmed.startsWith("⚠ ")) {
      raw = trimmed.slice(2).trim();
    } else if (trimmed === "■ 已中止" || trimmed.endsWith("■ 已中止")) {
      raw = "已中止";
    } else if (status === "aborted" || stopReason === "aborted") {
      raw = "已中止";
    }
  }

  if (!raw) {
    if (status === "aborted" || stopReason === "aborted") {
      raw = "已中止";
    } else {
      raw = "Agent 返回未知错误";
    }
  }

  const lower = raw.toLowerCase();

  // 1. 用户主动中止
  if (
    status === "aborted" ||
    stopReason === "aborted" ||
    raw === "已中止" ||
    raw.includes("用户中止") ||
    raw.includes("已中止")
  ) {
    return {
      category: "aborted",
      title: "生成已由用户中止",
      badge: "已中止",
      rawError: raw,
    };
  }

  // 2. 上下文长度超限（优先判定，防止 tokens 与认证 token 冲突）
  if (
    lower.includes("context length") ||
    lower.includes("context window") ||
    lower.includes("maximum context") ||
    lower.includes("too many tokens") ||
    lower.includes("prompt is too long") ||
    lower.includes("tokens exceed") ||
    lower.includes("context_length_exceeded") ||
    lower.includes("max_tokens") ||
    lower.includes("上下文") ||
    lower.includes("超出最大长度") ||
    lower.includes("超出长度")
  ) {
    return {
      category: "context_overflow",
      title: "会话上下文超出模型窗口限制",
      badge: "上下文超限",
      rawError: raw,
      suggestion: "当前会话历史已超出单次最大 Token 容纳窗口，建议在右侧面板执行「会话压缩」或开启新会话。",
    };
  }

  // 3. 速率与配额限制
  if (
    lower.includes("429") ||
    lower.includes("rate limit") ||
    lower.includes("rate_limit") ||
    lower.includes("quota") ||
    lower.includes("insufficient_quota") ||
    lower.includes("too many requests") ||
    lower.includes("tokens per minute") ||
    lower.includes("requests per minute") ||
    lower.includes("tpm") ||
    lower.includes("rpm") ||
    lower.includes("配额") ||
    lower.includes("超额") ||
    lower.includes("频率") ||
    lower.includes("限流") ||
    lower.includes("服务端要求")
  ) {
    return {
      category: "rate_limit",
      title: "请求频率超限或配额耗尽",
      badge: "频率限制",
      rawError: raw,
      suggestion: "服务端接口调用频率已达上限或账户额度耗尽，请稍候片刻重试，或切换备用模型。",
    };
  }

  // 4. 认证与密钥异常
  if (
    lower.includes("401") ||
    lower.includes("403") ||
    lower.includes("unauthorized") ||
    lower.includes("forbidden") ||
    lower.includes("invalidauthentication") ||
    lower.includes("invalid api key") ||
    lower.includes("incorrect api key") ||
    lower.includes("invalid_api_key") ||
    lower.includes("api key") ||
    lower.includes("apikey") ||
    lower.includes("auth_token") ||
    lower.includes("access_token") ||
    lower.includes("bearer") ||
    lower.includes("pipi_auth_token") ||
    lower.includes("invalid token") ||
    lower.includes("missing token") ||
    lower.includes("token is invalid") ||
    lower.includes("未授权") ||
    lower.includes("认证") ||
    lower.includes("鉴权")
  ) {
    return {
      category: "auth",
      title: "API 认证失败或凭据无效",
      badge: "认证异常",
      rawError: raw,
      suggestion: "请前往「设置 → 模型服务」检查 API 密钥、权限或端点凭据配置。",
    };
  }

  // 5. 协议不兼容
  if (
    lower.includes("modelprotocolunsupported") ||
    lower.includes("does not support this protocol") ||
    lower.includes("协议已固定") ||
    lower.includes("协议均已尝试") ||
    lower.includes("改为「自动」模式") ||
    lower.includes("协议不匹配")
  ) {
    return {
      category: "protocol",
      title: "模型请求协议不兼容",
      badge: "协议错误",
      rawError: raw,
      suggestion: "当前模型端点不支持所选协议格式，建议在设置中将该提供商的协议模式切换为「自动」。",
    };
  }

  // 6. 权限策略与安全拦截
  if (
    lower.includes("permission denied") ||
    lower.includes("权限拦截") ||
    lower.includes("命令被拒绝") ||
    lower.includes("安全策略") ||
    lower.includes("沙箱重定向")
  ) {
    return {
      category: "permission",
      title: "命令执行被安全策略拦截",
      badge: "权限拦截",
      rawError: raw,
      suggestion: "检测到可能对系统存在风险的命令或未授权的文件写入路径，已被安全防护机制拦截。",
    };
  }

  // 7. 网络连接与超时
  if (
    lower.includes("408") ||
    lower.includes("timeout") ||
    lower.includes("timed out") ||
    lower.includes("connection") ||
    lower.includes("connect") ||
    lower.includes("econnrefused") ||
    lower.includes("etimedout") ||
    lower.includes("流提前结束") ||
    lower.includes("连接被中断") ||
    lower.includes("连接重置") ||
    lower.includes("socket") ||
    lower.includes("fetch failed") ||
    lower.includes("网络") ||
    lower.includes("network") ||
    lower.includes("dns") ||
    lower.includes("reset by peer")
  ) {
    return {
      category: "network",
      title: "网络传输超时或连接中断",
      badge: "连接异常",
      rawError: raw,
      suggestion: "与模型服务端的通信中断或握手超时，请检查本地网络代理配置、VPN 或服务端状态。",
    };
  }

  // 8. 通用运行时异常
  return {
    category: "generic",
    title: "Agent 运行异常",
    badge: "运行错误",
    rawError: raw,
    suggestion: "模型调用或执行环节遇到未预期异常，可尝试重新发送或检查模型服务状态。",
  };
}

/**
 * 从可能混杂了错误/中止标记的助手文本中，拆分出干净的生成正文与错误信息对象。
 */
export function extractAssistantContentAndError(entry: {
  text?: string;
  errorMessage?: string;
  status?: string;
  stopReason?: string;
}): {
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
    return { cleanText: rawText, errorInfo: null };
  }

  // 处理文本中的中止标记
  let cleanText = rawText;
  if (cleanText.endsWith("\n■ 已中止")) {
    cleanText = cleanText.slice(0, -"\n■ 已中止".length);
  } else if (cleanText === "■ 已中止") {
    cleanText = "";
  }

  // 处理纯错误占位符
  if (cleanText.startsWith("⚠ ")) {
    // 纯错误消息不作为正文
    cleanText = "";
  }

  const errorInfo = classifyChatError({
    errorMessage: entry.errorMessage,
    text: rawText,
    status: entry.status,
    stopReason: entry.stopReason,
  });

  return { cleanText, errorInfo };
}
