import { useEffect, useState } from "react";
import type { PromptQueueSnapshot } from "../types";
import { formatRuntimeError } from "../chat-runtime";
import { useI18n } from "../i18n";

interface Props {
  queue: PromptQueueSnapshot;
  onCancel: (id: string) => Promise<void>;
  onPause: (paused: boolean) => Promise<void>;
  onError: (message: string) => void;
}

/** 待发消息不冒充已经进入会话的 user entry。 */
export default function ComposerQueue({ queue, onCancel, onPause, onError }: Props) {
  const { locale, t } = useI18n();
  const en = locale === "en";
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!queue.messages.length) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [queue.messages.length]);
  if (!queue.messages.length) return null;
  const act = async (action: () => Promise<void>) => {
    setBusy(true);
    try { await action(); } catch (error) { onError(formatRuntimeError(error)); }
    finally { setBusy(false); }
  };
  return (
    <section className="composer-queue" aria-label={t("chat.pendingMessages")}>
      <div className="composer-queue-header">
        <strong>{t("chat.pendingCount", { count: queue.messages.length })}</strong>
        <span role="status">{queue.paused ? t("chat.queuePaused") : t("chat.sendWhenIdle")}</span>
        <button type="button" className="btn" disabled={busy} onClick={() => void act(() => onPause(!queue.paused))}>
          {queue.paused ? t("chat.resumeQueue") : t("chat.pauseQueue")}
        </button>
      </div>
      {queue.error && <p className="composer-queue-error" role="status">{queue.error}</p>}
      <ol>
        {queue.messages.map(message => (
          <li key={message.id}>
            <div className="composer-queue-message">
              <span className="truncate composer-queue-text" title={message.text}>{message.text}</span>
              {!!message.imageCount && <small>{t("chat.imageCount", { count: message.imageCount })}</small>}
              <small>{message.sendAt > now
                ? t("chat.sendAfter", {
                    time: new Date(message.sendAt).toLocaleString(en ? "en-US" : "zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" }),
                  })
                : t("chat.waitingForIdle")}</small>
            </div>
            <button type="button" className="btn" disabled={busy} aria-label={t("chat.cancelQueued", { text: message.text })} onClick={() => void act(() => onCancel(message.id))}>
              {t("common.cancel")}
            </button>
          </li>
        ))}
      </ol>
      <p className="composer-queue-note">{t("chat.queueNote")}</p>
    </section>
  );
}
