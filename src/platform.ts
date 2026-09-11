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

const TOKEN_STORAGE_KEY = "pipi_web_token";

/** 前端可见的运行期连接状态（状态栏用）。 */
export type ConnectionState = "online" | "connecting" | "offline" | "dev";

const connectionListeners = new Set<(state: ConnectionState) => void>();
let webConnectionState: ConnectionState = "connecting";

function setConnectionState(next: ConnectionState): void {
  if (webConnectionState === next) return;
  webConnectionState = next;
  connectionListeners.forEach((listener) => listener(next));
}

/**
 * 桌面端与开发桩没有「连接」概念，直接视为在线；
 * 浏览器模式跟随 WebSocket 状态。
 */
export function getConnectionState(): ConnectionState {
  if (devPlatform()) return "dev";
  if (isTauriRuntime()) return "online";
  return webConnectionState;
}

export function subscribeConnection(listener: (state: ConnectionState) => void): () => void {
  connectionListeners.add(listener);
  listener(getConnectionState());
  return () => {
    connectionListeners.delete(listener);
  };
}

/** 状态栏展示用：当前实际生效的运行端点描述。 */
export function getRuntimeEndpoint(): string {
  if (devPlatform()) return "浏览器演示桩";
  if (isTauriRuntime()) return "桌面端 · 内置核心";
  try {
    return `本地服务 ${new URL(webApiBase()).host}`;
  } catch {
    return "本地服务";
  }
}

export interface AuthStatus {
  authRequired: boolean;
  authenticated: boolean;
}

type AuthRequiredListener = () => void;
const authRequiredListeners = new Set<AuthRequiredListener>();

export function onAuthRequired(listener: AuthRequiredListener): () => void {
  authRequiredListeners.add(listener);
  return () => {
    authRequiredListeners.delete(listener);
  };
}

export function notifyAuthRequired(): void {
  authRequiredListeners.forEach((fn) => fn());
}

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined"
    && "__TAURI_INTERNALS__" in (window as Window & { __TAURI_INTERNALS__?: unknown });
}

export function getStoredToken(): string | null {
  if (typeof window === "undefined") return null;
  try {
    const query = new URLSearchParams(window.location.search);
    const queryToken = query.get("token");
    if (queryToken) {
      localStorage.setItem(TOKEN_STORAGE_KEY, queryToken);
      const url = new URL(window.location.href);
      url.searchParams.delete("token");
      window.history.replaceState({}, "", url.toString());
      return queryToken;
    }
  } catch {}
  try {
    return localStorage.getItem(TOKEN_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function setStoredToken(token: string | null): void {
  if (typeof window === "undefined") return;
  try {
    if (token) {
      localStorage.setItem(TOKEN_STORAGE_KEY, token);
    } else {
      localStorage.removeItem(TOKEN_STORAGE_KEY);
    }
  } catch {}
}

function webApiBase(): string {
  const configured = (typeof import.meta !== "undefined" && import.meta.env?.VITE_PIPI_API_BASE)?.trim();
  return (configured || (typeof window !== "undefined" ? window.location.origin : "")).replace(/\/+$/, "");
}

function webToken(): string | null {
  return getStoredToken();
}

function webHeaders(): HeadersInit {
  const token = webToken();
  return token
    ? { "Content-Type": "application/json", "X-Pipi-Token": token }
    : { "Content-Type": "application/json" };
}

export async function getAuthStatus(): Promise<AuthStatus> {
  if (isTauriRuntime()) {
    return { authRequired: false, authenticated: true };
  }
  try {
    const response = await fetch(webApiBase() + "/api/auth/status", {
      method: "GET",
      headers: webHeaders(),
      credentials: "include",
    });
    if (response.ok) {
      const data = (await response.json()) as AuthStatus;
      return data;
    }
  } catch {}
  return { authRequired: false, authenticated: true };
}

export async function loginWithToken(token: string): Promise<{ ok: boolean; error?: string }> {
  try {
    const response = await fetch(webApiBase() + "/api/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "include",
      body: JSON.stringify({ token: token.trim() }),
    });
    const data = (await response.json().catch(() => ({}))) as { ok?: boolean; error?: string };
    if (response.ok && data.ok) {
      setStoredToken(token.trim());
      closeWebSocket();
      ensureWebSocket();
      return { ok: true };
    }
    return { ok: false, error: data.error || "Token 错误，请核对后重试" };
  } catch {
    return { ok: false, error: "连接服务器失败，请检查网络" };
  }
}

export async function logout(): Promise<void> {
  setStoredToken(null);
  try {
    await fetch(webApiBase() + "/api/auth/logout", {
      method: "POST",
      credentials: "include",
    });
  } catch {}
  closeWebSocket();
  notifyAuthRequired();
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
    if (response.status === 401) {
      notifyAuthRequired();
      const authErr = new Error("需要 PIPI_AUTH_TOKEN");
      (authErr as unknown as { isAuthError?: boolean }).isAuthError = true;
      throw authErr;
    }
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
  setConnectionState("connecting");
  socket.onopen = () => {
    webReconnectAttempt = 0;
    setConnectionState("online");
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
  socket.onerror = () => {
    setConnectionState("offline");
    socket.close();
  };
  socket.onclose = () => {
    if (webSocket !== socket) return;
    webSocket = null;
    setConnectionState("offline");
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
