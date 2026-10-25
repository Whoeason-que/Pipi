/* Notification worker only: authenticated API responses and conversations are never cached. */
self.addEventListener("install", event => event.waitUntil(self.skipWaiting()));
self.addEventListener("activate", event => event.waitUntil(self.clients.claim()));

self.addEventListener("push", event => {
  let notice = {};
  try { notice = event.data?.json() ?? {}; } catch { /* Always display a visible notification. */ }
  if (!notice || typeof notice !== "object") notice = {};
  const title = typeof notice.title === "string" ? notice.title.slice(0, 150) : "Pipi";
  const body = typeof notice.body === "string" ? notice.body.slice(0, 250) : "An update is available in Pipi";
  event.waitUntil(self.registration.showNotification(title, {
    body,
    icon: "/icon-256.png",
    badge: "/icon-128.png",
    tag: typeof notice.id === "string" ? notice.id : "pipi-update",
    data: { agentName: notice.agentName, sessionId: notice.sessionId, runId: notice.runId },
  }));
});

self.addEventListener("notificationclick", event => {
  event.notification.close();
  const data = event.notification.data ?? {};
  const valid = typeof data.agentName === "string" && data.agentName.length > 0 && data.agentName.length <= 200
    && typeof data.sessionId === "string" && /^[a-zA-Z0-9_-]{1,200}$/.test(data.sessionId);
  const url = new URL("/", self.location.origin);
  if (valid) {
    url.searchParams.set("notificationAgent", data.agentName);
    url.searchParams.set("notificationSession", data.sessionId);
  }
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    const client = windows.find(client => new URL(client.url).origin === self.location.origin);
    if (client) {
      if (valid) client.postMessage({ type: "pipi-notification-open", notification: data });
      await client.focus();
    } else {
      await self.clients.openWindow(url.href);
    }
  })());
});
