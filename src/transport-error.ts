/** 请求可能已被宿主接收；调用方不得把传输失败当作命令拒绝。 */
export class CommandTransportError extends Error {
  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
    this.name = "CommandTransportError";
  }
}


/** 宿主只用 HTTP 400 + {error: string} 返回命令拒绝；代理错误不能证明执行失败。 */
export function decodeCommandResponse(status: number, text: string): unknown {
  let value: unknown = null;
  if (text) {
    try { value = JSON.parse(text); }
    catch { throw new CommandTransportError(`Invalid command response (HTTP ${status})`); }
  }
  if (status >= 200 && status < 300) return value;
  if (status === 400 && value && typeof value === "object" && "error" in value
    && typeof value.error === "string") throw new Error(value.error);
  if (status === 401 || status === 403) throw new Error(`Request denied (HTTP ${status})`);
  throw new CommandTransportError(`Command response unavailable (HTTP ${status})`);
}
