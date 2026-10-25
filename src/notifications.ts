/** 系统通知只允许导航到同一个 Pipi 里的会话，不接受服务端提供的任意 URL。 */
export interface NotificationEnvelope {
  agentName: string;
  sessionId: string;
  runId: number;
  id: string;
  kind: "completed" | "failed";
  title: string;
  body: string;
}

export interface NotificationTarget { agentName: string; sessionId: string }

export function notificationTarget(value: unknown): NotificationTarget | null {
  if (!value || typeof value !== "object") return null;
  const data = value as Record<string, unknown>;
  if (typeof data.agentName !== "string" || !data.agentName || data.agentName.length > 200
    || typeof data.sessionId !== "string" || !/^[a-zA-Z0-9_-]{1,200}$/.test(data.sessionId)) return null;
  return { agentName: data.agentName, sessionId: data.sessionId };
}

export function notificationTargetFromUrl(url: URL): NotificationTarget | null {
  return notificationTarget({ agentName: url.searchParams.get("notificationAgent"), sessionId: url.searchParams.get("notificationSession") });
}

export function decodeApplicationServerKey(value: string): Uint8Array<ArrayBuffer> {
  const text = atob(value.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(text, character => character.charCodeAt(0));
}
