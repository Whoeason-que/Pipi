import { useEffect, useState } from "react";
import { errorMessage } from "../errors.ts";
import { useI18n } from "../i18n";
import { invoke, isTauriRuntime } from "../platform";
import { currentPushSubscription, disableWebPush, enableWebPush, supportsWebPush } from "../notifications-client";
import type { NotificationSettings } from "../types";

export default function NotificationsPanel({ settings, onChange }: { settings: NotificationSettings; onChange: (settings: NotificationSettings) => void }) {
  const { t } = useI18n();
  const desktop = isTauriRuntime();
  const [subscribed, setSubscribed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const supported = desktop || supportsWebPush();
  const refresh = async () => {
    const subscription = await currentPushSubscription();
    setSubscribed(Boolean(subscription && await invoke("push_subscription_status", { endpoint: subscription.endpoint })));
  };
  useEffect(() => {
    if (!desktop && supported) void refresh().catch(reason => setError(errorMessage(reason)));
  }, [desktop, supported]);
  const perform = async (operation: () => Promise<void>) => {
    setBusy(true); setError(null); setNote(null);
    try { await operation(); }
    catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  };
  return <section className="settings-block">
    <span className="label">{t("notifications.thisDevice")}</span>
    <p className="hint">{t(desktop ? "notifications.desktopHint" : "notifications.webHint")}</p>
    {desktop ? <label className="ios-toggle settings-toggle">
      <input type="checkbox" checked={settings.desktopEnabled} onChange={() => onChange({ ...settings, desktopEnabled: !settings.desktopEnabled })} />
      <span className="slider" /><span className="toggle-label"><strong>{t("notifications.desktopEnabled")}</strong><span>{t("notifications.backgroundOnly")}</span></span>
    </label> : <>
      <p role="status">{t(subscribed ? "notifications.subscribed" : "notifications.unsubscribed")}</p>
      {!supported && <p className="hint">{t("notifications.unsupported")}</p>}
      {typeof Notification !== "undefined" && Notification.permission === "denied" && <p className="hint">{t("notifications.denied")}</p>}
      <div className="right-rail-actions-row">
        <button className="btn" disabled={busy || !supported} onClick={() => void perform(async () => { await enableWebPush(); await refresh(); })}>
          {t(subscribed ? "notifications.reconnect" : "notifications.enable")}
        </button>
        {subscribed && <button className="btn ghost" disabled={busy} onClick={() => void perform(async () => { await disableWebPush(); await refresh(); })}>{t("notifications.disable")}</button>}
      </div>
    </>}
    <button className="btn ghost" disabled={busy || (!desktop && !subscribed)} onClick={() => void perform(async () => {
      if (desktop) await invoke("test_desktop_notification");
      else {
        const subscription = await currentPushSubscription();
        if (!subscription) throw new Error("This device is not subscribed.");
        await invoke("test_push_notification", { endpoint: subscription.endpoint });
      }
      setNote(t("notifications.testSent"));
    })}>{t("notifications.test")}</button>
    {error && <p role="alert" className="error-banner">{error}</p>}
    {note && <p role="status" className="hint">{note}</p>}
    <span className="label">{t("notifications.categories")}</span>
    <div className="setting-checks">
      {(["completed", "failed"] as const).map(kind => <label className="ios-toggle settings-toggle" key={kind}>
        <input type="checkbox" checked={settings[kind]} onChange={() => onChange({ ...settings, [kind]: !settings[kind] })} />
        <span className="slider" /><span className="toggle-label"><strong>{t(`notifications.${kind}`)}</strong></span>
      </label>)}
    </div>
    <p className="hint">{t("notifications.privacy")}</p>
  </section>;
}
