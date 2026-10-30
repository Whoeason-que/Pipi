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
  const { locale } = useI18n();
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
    <section className="composer-queue" aria-label={en ? "Pending messages" : "待发消息"}>
      <div className="composer-queue-header">
        <strong>{en ? `Pending · ${queue.messages.length}` : `待发消息 · ${queue.messages.length}`}</strong>
        <span role="status">{queue.paused ? (en ? "Paused" : "已暂停") : (en ? "Send when idle" : "空闲时逐条发送")}</span>
        <button type="button" className="btn" disabled={busy} onClick={() => void act(() => onPause(!queue.paused))}>
          {queue.paused ? (en ? "Resume" : "继续队列") : (en ? "Pause" : "暂停队列")}
        </button>
      </div>
      {queue.error && <p className="composer-queue-error" role="status">{queue.error}</p>}
      <ol>
        {queue.messages.map(message => (
          <li key={message.id}>
            <div className="composer-queue-message">
              <span className="truncate composer-queue-text" title={message.text}>{message.text}</span>
              {!!message.imageCount && <small>{en ? `${message.imageCount} images` : `${message.imageCount} 张图片`}</small>}
              <small>{message.sendAt > now
                ? `${en ? "After " : "最早 "}${new Date(message.sendAt).toLocaleString(en ? "en-US" : "zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" })}`
                : (en ? "Waiting for an idle session" : "等待会话空闲")}</small>
            </div>
            <button type="button" className="btn" disabled={busy} aria-label={en ? `Cancel: ${message.text}` : `取消：${message.text}`} onClick={() => void act(() => onCancel(message.id))}>
              {en ? "Cancel" : "取消"}
            </button>
          </li>
        ))}
      </ol>
      <p className="composer-queue-note">{en ? "Runs on this host. Quitting Pipi clears pending messages." : "由宿主执行；退出 Pipi 会清空待发消息。"}</p>
    </section>
  );
}
