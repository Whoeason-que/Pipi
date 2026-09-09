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

/**
 * Pipi 的唯一前端运行时边界：桌面端走 Tauri，浏览器开发模式走本地 mock。
 * 组件不再直接依赖 Tauri API，事件流因此可以在两种运行环境中保持一致。
 */
export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const dev = devPlatform();
  return dev ? dev.invoke<T>(command, args) : tauriInvoke<T>(command, args);
}

export function listen<T>(
  event: string,
  listener: (event: PlatformEvent<T>) => void,
): Promise<UnlistenFn> {
  const dev = devPlatform();
  if (dev) {
    return dev.listen(event, listener as (event: PlatformEvent<unknown>) => void);
  }
  return tauriListen<T>(event, (payload) => listener(payload));
}
