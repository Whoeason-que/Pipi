import { useCopyFeedback } from "../components/use-copy";
import { useState } from "react";
import type { ChatErrorInfo } from "./chat-errors";
import { useI18n } from "../i18n";
import {
  IconAlertTriangle,
  IconArrowRepeat,
  IconCheck,
  IconClock,
  IconCopy,
  IconKey,
  IconLayers,
  IconShieldAlert,
  IconStop,
  IconWifiOff,
} from "../icons";

function renderCategoryIcon(category: ChatErrorInfo["category"]) {
  switch (category) {
    case "aborted":
      return <IconStop size={12} />;
    case "auth":
      return <IconKey size={14} />;
    case "quota":
    case "rate_limit":
      return <IconClock size={14} />;
    case "context_overflow":
      return <IconLayers size={14} />;
    case "network":
      return <IconWifiOff size={14} />;
    case "protocol":
      return <IconArrowRepeat size={14} />;
    case "permission":
      return <IconShieldAlert size={14} />;
    case "generic":
    default:
      return <IconAlertTriangle size={14} />;
  }
}

export function ChatErrorCard({ info }: { info: ChatErrorInfo }) {
  const { t } = useI18n();
  const clipboard = useCopyFeedback();
  const copied = clipboard.state === "copied";
  const [openRaw, setOpenRaw] = useState(false);

  // 用户主动中止：以温和低调的 pill 胶囊展现，不给红色惊悚感
  if (info.category === "aborted") {
    return (
      <div className="chat-aborted-pill" role="status" aria-label={info.title}>
        <span className="chat-aborted-icon">
          <IconStop size={11} />
        </span>
        <span className="chat-aborted-text">{info.title}</span>
      </div>
    );
  }

  const handleCopy = () => clipboard.copy(`${info.title}\n[${info.badge}] ${info.rawError}${
    info.suggestion ? `\n${t("chat.suggestionPrefix")}${info.suggestion}` : ""
  }`);
  const copyLabel = clipboard.state === "failed" ? t("chat.copyFailed")
    : copied ? t("common.copied") : t("chat.copyError");

  const isLongError = info.rawError.length > 70 || info.rawError.includes("\n");

  return (
    <div
      className={`chat-error-card chat-error-${info.category}`}
      role="alert"
      aria-label={`${info.title}：${info.badge}`}
    >
      <div className="chat-error-head">
        <div className="chat-error-icon">{renderCategoryIcon(info.category)}</div>
        <div className="chat-error-title-wrap">
          <span className="chat-error-title">{info.title}</span>
          <span className="chat-error-badge">{info.badge}</span>
        </div>
        <button
          type="button"
          className="chat-error-copy-btn"
          onClick={handleCopy}
          title={copyLabel}
          aria-label={copyLabel}
        >
          {copied ? <IconCheck /> : <IconCopy />}
          <span className="copy-label">{clipboard.state === "failed" ? t("chat.copyFailed") : copied ? t("common.copied") : t("common.copy")}</span>
        </button>
      </div>

      {info.suggestion && (
        <div className="chat-error-suggestion">
          <span className="suggestion-prefix">{t("chat.suggestionPrefix")}</span>
          <span className="suggestion-text">{info.suggestion}</span>
        </div>
      )}

      {isLongError ? (
        <details
          className="chat-error-details"
          open={openRaw}
          onToggle={(e) => setOpenRaw(e.currentTarget.open)}
        >
          <summary className="chat-error-summary">
            <span>{openRaw ? t("chat.collapseDetails") : t("chat.expandDetails")}</span>
          </summary>
          <pre className="chat-error-pre">{info.rawError}</pre>
        </details>
      ) : (
        <div className="chat-error-message">{info.rawError}</div>
      )}
    </div>
  );
}
