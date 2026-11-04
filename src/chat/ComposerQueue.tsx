import { useEffect, useState } from "react";
import type { PromptQueueSnapshot } from "../types";
import { formatRuntimeError } from "../chat-runtime";
import { useI18n } from "../i18n";
import { IconAlertTriangle, IconClock, IconClose } from "../icons";

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
  // 未到时的消息单独成组；到时后自动并入当前队列，由队列按空闲状态发送。
  const scheduled = queue.messages.filter((message) => message.sendAt > now);
  const ready = queue.messages.filter((message) => message.sendAt <= now);
  const act = async (action: () => Promise<void>) => {
    setBusy(true);
    try { await action(); } catch (error) { onError(formatRuntimeError(error)); }
    finally { setBusy(false); }
  };
  const renderRow = (message: PromptQueueSnapshot["messages"][number], index: number) => (
    <li key={message.id}>
      <span className="composer-queue-index" aria-hidden="true">{index + 1}</span>
      <span className="composer-queue-message">
        <span className="truncate fade-end composer-queue-text" title={message.text}>{message.text}</span>
        <small className="composer-queue-meta">
          {!!message.imageCount && <span>{t("chat.imageCount", { count: message.imageCount })}</span>}
          <span>{message.sendAt > now
            ? t("chat.sendAfter", {
                time: new Date(message.sendAt).toLocaleString(en ? "en-US" : "zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" }),
              })
            : t("chat.waitingForIdle")}</span>
        </small>
      </span>
      <button
        type="button"
        className="icon-btn composer-queue-cancel"
        disabled={busy}
        aria-label={t("chat.cancelQueued", { text: message.text })}
        title={t("chat.cancelQueued", { text: message.text })}
        onClick={() => void act(() => onCancel(message.id))}
      >
        <IconClose />
      </button>
    </li>
  );
  return (
    <section
      className={`composer-queue${queue.paused ? " is-paused" : ""}`}
      aria-label={t("chat.pendingMessages")}
      title={t("chat.queueNote")}
    >
      <div className="composer-queue-head">
        <span className="composer-queue-title">
          <IconClock size={13} />
          <strong>{t("chat.queueCount", { count: queue.messages.length })}</strong>
        </span>
        <span className="composer-queue-status" role="status">
          {queue.paused
            ? t("chat.queuePaused")
            : ready.length
              ? t("chat.sendWhenIdle")
              : t("chat.scheduledStatus")}
        </span>
        <button
          type="button"
          className="btn small ghost composer-queue-toggle"
          disabled={busy}
          onClick={() => void act(() => onPause(!queue.paused))}
        >
          {queue.paused ? t("chat.resumeQueue") : t("chat.pauseQueue")}
        </button>
      </div>
      {queue.error && (
        <p className="composer-queue-error" role="status">
          <IconAlertTriangle size={13} />
          <span>{queue.error}</span>
        </p>
      )}
      <div className="composer-queue-list">
        {scheduled.length > 0 && (
          <section className="composer-queue-group">
            <p className="composer-queue-group-label">
              {t("chat.scheduledCount", { count: scheduled.length })}
            </p>
            <ol>{scheduled.map(renderRow)}</ol>
          </section>
        )}
        {ready.length > 0 && (
          <section className="composer-queue-group">
            <p className="composer-queue-group-label">
              {t("chat.currentQueueCount", { count: ready.length })}
            </p>
            <ol>{ready.map(renderRow)}</ol>
          </section>
        )}
      </div>
    </section>
  );
}
