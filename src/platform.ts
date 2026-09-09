import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type Event, type UnlistenFn } from "@tauri-apps/api/event";

type PlatformEvent<T> = Pick<Event<T>, "payload">;
type PlatformListener = (event: PlatformEvent<unknown>) => void;

interface DevPlatform {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen(event: string, listener: PlatformListener): Promise<UnlistenFn>;
}

declare global {
  interface Window {
    __PIPI_DEV_PLATFORM__?: DevPlatform;
  }
}

function devPlatform(): DevPlatform | undefined {
  return typeof window === "undefined" ? undefined : window.__PIPI_DEV_PLATFORM__;
}

type WebEventListener = (event: PlatformEvent<unknown>) => void;

const webListeners = new Map<string, Set<WebEventListener>>();
let webSocket: WebSocket | null = null;
let webReconnectTimer: number | null = null;
let webReconnectAttempt = 0;

function isTauriRuntime(): boolean {
  return typeof window !== "undefined"
    && "__TAURI_INTERNALS__" in (window as Window & { __TAURI_INTERNALS__?: unknown });
}

function webApiBase(): string {
  const configured = import.meta.env.VITE_PIPI_API_BASE?.trim();
  return (configured || window.location.origin).replace(/\/+$/, "");
}

function webToken(): string | null {
  return new URLSearchParams(window.location.search).get("token");
}

function webHeaders(): HeadersInit {
  const token = webToken();
  return token
    ? { "Content-Type": "application/json", "X-Pipi-Token": token }
    : { "Content-Type": "application/json" };
}

async function webInvoke<T>(
  command: string,
  args: Record<string, unknown> | undefined,
): Promise<T> {
  const response = await fetch(webApiBase() + "/api/invoke", {
    method: "POST",
    headers: webHeaders(),
    credentials: "include",
    body: JSON.stringify({ command, args: args ?? {} }),
  });
  const text = await response.text();
  let value: unknown = null;
  if (text) {
    try {
      value = JSON.parse(text);
    } catch {
      value = text;
    }
  }
  if (!response.ok) {
    const message = value && typeof value === "object" && "error" in value
      ? (value as { error?: unknown }).error
      : value;
    throw new Error(
      typeof message === "string" ? message : "请求失败（HTTP " + response.status + "）",
    );
  }
  return value as T;
}

function webEventUrl(): string {
  const url = new URL(webApiBase() + "/api/events");
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  const token = webToken();
  if (token) url.searchParams.set("token", token);
  return url.toString();
}

function closeWebSocket(): void {
  if (webReconnectTimer !== null) {
    window.clearTimeout(webReconnectTimer);
    webReconnectTimer = null;
  }
  if (webSocket) {
    const current = webSocket;
    webSocket = null;
    current.close();
  }
}

function scheduleWebSocketReconnect(): void {
  if (webListeners.size === 0 || webReconnectTimer !== null) return;
  const delay = Math.min(1000 * 2 ** webReconnectAttempt, 10_000);
  webReconnectAttempt += 1;
  webReconnectTimer = window.setTimeout(() => {
    webReconnectTimer = null;
    ensureWebSocket();
  }, delay);
}

function ensureWebSocket(): void {
  if (webListeners.size === 0) return;
  if (
    webSocket
    && (webSocket.readyState === WebSocket.CONNECTING || webSocket.readyState === WebSocket.OPEN)
  ) {
    return;
  }
  const socket = new WebSocket(webEventUrl());
  webSocket = socket;
  socket.onopen = () => {
    webReconnectAttempt = 0;
  };
  socket.onmessage = (message) => {
    if (typeof message.data !== "string") return;
    try {
      const frame = JSON.parse(message.data) as {
        type?: string;
        payload?: unknown;
      };
      if (!frame.type || !("payload" in frame)) return;
      webListeners.get(frame.type)?.forEach((listener) => listener({ payload: frame.payload }));
    } catch {
      // 忽略无法解析的远程帧；下一帧仍可继续处理。
    }
  };
  socket.onerror = () => socket.close();
  socket.onclose = () => {
    if (webSocket !== socket) return;
    webSocket = null;
    scheduleWebSocketReconnect();
  };
}

function webListen(
  event: string,
  listener: WebEventListener,
): Promise<UnlistenFn> {
  const listeners = webListeners.get(event) ?? new Set<WebEventListener>();
  listeners.add(listener);
  webListeners.set(event, listeners);
  ensureWebSocket();
  const unlisten: UnlistenFn = () => {
    const current = webListeners.get(event);
    current?.delete(listener);
    if (current?.size === 0) webListeners.delete(event);
    if (webListeners.size === 0) closeWebSocket();
  };
  return Promise.resolve(unlisten);
}

/**
 * Pipi 的唯一前端运行时边界：Tauri 桌面端走 IPC，普通浏览器走
 * HTTP/WebSocket，开发环境可注入本地 mock。组件不依赖具体传输方式。
 */
export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const dev = devPlatform();
  if (dev) return dev.invoke<T>(command, args);
  return isTauriRuntime()
    ? tauriInvoke<T>(command, args)
    : webInvoke<T>(command, args);
}

export function listen<T>(
  event: string,
  listener: (event: PlatformEvent<T>) => void,
): Promise<UnlistenFn> {
  const dev = devPlatform();
  if (dev) {
    return dev.listen(event, listener as (event: PlatformEvent<unknown>) => void);
  }
  if (isTauriRuntime()) {
    return tauriListen<T>(event, (payload) => listener(payload));
  }
  return webListen(event, listener as WebEventListener);
}
