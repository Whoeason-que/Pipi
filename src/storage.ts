/**
 * 浏览器存储访问器：隐私模式/禁用存储时返回 undefined，调用方继续走内存降级。
 *
 * 只负责「取到可用的 Storage」，不搬各模块的 JSON schema 与版本迁移。
 */
export function safeSessionStorage(): Storage | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.sessionStorage;
  } catch {
    return undefined;
  }
}

export function safeLocalStorage(): Storage | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.localStorage;
  } catch {
    return undefined;
  }
}
