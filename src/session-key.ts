/**
 * 复合会话键：(Agent 名, 会话 id)。
 *
 * 会话在宿主与前端索引里都按这一对定位；键格式只在这里定义，避免四处各写
 * 一份分隔符后改一处漏三处。`\u0000` 不会出现在合法的 Agent 名/会话 id 里。
 */
export function sessionKey(agentName: string, sessionId: string): string {
  return `${agentName}\u0000${sessionId}`;
}

/** 某 Agent 的全部会话键前缀（按 Agent 批量清理/筛选用）。 */
export function sessionKeyPrefix(agentName: string): string {
  return `${agentName}\u0000`;
}

/** 把复合键拆回 `[Agent 名, 会话 id]`。 */
export function splitSessionKey(key: string): [string, string] {
  const index = key.indexOf("\u0000");
  return index < 0 ? [key, ""] : [key.slice(0, index), key.slice(index + 1)];
}
