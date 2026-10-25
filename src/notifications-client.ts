import { invoke } from "./platform.ts";
import { decodeApplicationServerKey } from "./notifications.ts";

export function supportsWebPush(): boolean {
  return window.isSecureContext && "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
}

export async function registerNotificationWorker(): Promise<ServiceWorkerRegistration> {
  await navigator.serviceWorker.register("/sw.js", { scope: "/", updateViaCache: "none" });
  return navigator.serviceWorker.ready;
}

export async function currentPushSubscription(): Promise<PushSubscription | null> {
  if (!supportsWebPush()) return null;
  const registration = await navigator.serviceWorker.getRegistration("/");
  return registration?.pushManager.getSubscription() ?? null;
}

export async function enableWebPush(): Promise<PushSubscription> {
  if (!supportsWebPush()) throw new Error("Web Push requires HTTPS and a supported Home Screen web app.");
  // 必须在点击处理栈里请求权限，不能先 await 网络或 Service Worker 注册。
  const permission = await Notification.requestPermission();
  if (permission !== "granted") throw new Error("Notification permission was not granted. Check system notification settings.");
  const registration = await registerNotificationWorker();
  const publicKey = await invoke("web_push_public_key");
  const applicationServerKey = decodeApplicationServerKey(publicKey);
  let subscription = await registration.pushManager.getSubscription();
  const registered = subscription && await invoke("push_subscription_status", { endpoint: subscription.endpoint });
  if (subscription && (!registered || (subscription.options.applicationServerKey
    && Array.from(new Uint8Array(subscription.options.applicationServerKey)).join() !== Array.from(applicationServerKey).join()))) {
    await invoke("remove_push_subscription", { endpoint: subscription.endpoint });
    await subscription.unsubscribe();
    subscription = null;
  }
  const created = !subscription;
  subscription ??= await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey });
  const data = subscription.toJSON();
  try {
    if (!data.keys?.p256dh || !data.keys.auth) throw new Error("Browser returned an incomplete push subscription.");
    await invoke("register_push_subscription", {
      endpoint: subscription.endpoint, p256dh: data.keys.p256dh, auth: data.keys.auth, origin: location.origin,
    });
  } catch (error) {
    if (created) await subscription.unsubscribe();
    throw error;
  }
  return subscription;
}

export async function disableWebPush(): Promise<void> {
  const subscription = await currentPushSubscription();
  if (!subscription) return;
  await invoke("remove_push_subscription", { endpoint: subscription.endpoint });
  await subscription.unsubscribe();
}
