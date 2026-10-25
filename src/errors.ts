/**
 * 把未知异常归一为可展示文本。
 *
 * 设置页、目录加载、会话控制器都可能拿到 Error / string / 宿主返回的
 * `{ message }` 对象；统一走这里，避免 `[object Object]` 或各处重复实现。
 */
export function errorMessage(error: unknown, fallback = "未知错误"): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return fallback;
}
