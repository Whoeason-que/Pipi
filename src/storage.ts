/**
 * 浏览器存储访问器：隐私模式/禁用存储时返回 undefined，调用方继续走内存降级。
 *
 * 只负责「取到可用的 Storage」与单键读写，不搬各模块的 JSON schema 与版本迁移。
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
    // 优先全局标识符：浏览器里它就是 window.localStorage，测试环境则可能只
    // 注入 globalThis.localStorage 而没有 window（两者可能不是同一个对象）。
    if (typeof localStorage !== "undefined") return localStorage;
    return typeof window === "undefined" ? undefined : window.localStorage;
  } catch {
    return undefined;
  }
}

/** 读一个 localStorage 键；存储不可用时返回 `fallback`。 */
export function readStored(key: string, fallback: string): string {
  try {
    return safeLocalStorage()?.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

/** 写一个 localStorage 键；存储不可用时静默忽略（调用方继续用内存态）。 */
export function writeStored(key: string, value: string): void {
  try {
    safeLocalStorage()?.setItem(key, value);
  } catch {
    // 隐私模式/配额耗尽：不影响本次使用
  }
}

/** 删一个 localStorage 键；存储不可用时静默忽略。 */
export function deleteStored(key: string): void {
  try {
    safeLocalStorage()?.removeItem(key);
  } catch {
    // 隐私模式/禁用存储：视为已经不存在
  }
}
