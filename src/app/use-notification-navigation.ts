import { useEffect, useRef } from "react";
import { listen } from "../platform";
import { notificationTarget, notificationTargetFromUrl, type NotificationTarget } from "../notifications";

/** 与 ChatView 生命周期无关，冷启动的通知目标等鉴权与 Agent 索引就绪后再打开。 */
export function useNotificationNavigation(ready: boolean, openSession: (agent: string, session: string) => Promise<void>) {
  const pending = useRef<NotificationTarget | null>(notificationTargetFromUrl(new URL(location.href)));
  const state = useRef({ ready, openSession });
  state.current = { ready, openSession };
  const flush = () => {
    if (!state.current.ready || !pending.current) return;
    const target = pending.current;
    pending.current = null;
    const url = new URL(location.href);
    url.searchParams.delete("notificationAgent");
    url.searchParams.delete("notificationSession");
    history.replaceState(history.state, "", url);
    void state.current.openSession(target.agentName, target.sessionId);
  };
  useEffect(() => { flush(); }, [ready]);
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    const receive = (value: unknown) => {
      if (!active) return;
      pending.current = notificationTarget(value);
      flush();
    };
    void listen("notification-open", event => receive(event.payload)).then(stop => {
      if (active) unlisten = stop; else stop();
    }).catch(() => {});
    const onMessage = (event: MessageEvent) => {
      if (event.data?.type === "pipi-notification-open") receive(event.data.notification);
    };
    navigator.serviceWorker?.addEventListener("message", onMessage);
    return () => {
      active = false;
      unlisten?.();
      navigator.serviceWorker?.removeEventListener("message", onMessage);
    };
  }, []);
}
