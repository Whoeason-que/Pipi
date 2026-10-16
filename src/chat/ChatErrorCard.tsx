import { useState } from "react";
import type { ChatErrorInfo } from "./chat-errors";
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
  const [copied, setCopied] = useState(false);
  const [openRaw, setOpenRaw] = useState(false);

  // 用户主动中止：以温和低调的 pill 胶囊展现，不给红色惊悚感
  if (info.category === "aborted") {
    return (
      <div className="chat-aborted-pill" role="status" aria-label="生成已中止">
        <span className="chat-aborted-icon">
          <IconStop size={11} />
        </span>
        <span className="chat-aborted-text">{info.title}</span>
      </div>
    );
  }

  const handleCopy = async () => {
    try {
      const textToCopy = `${info.title}\n[${info.badge}] ${info.rawError}${
        info.suggestion ? `\n建议: ${info.suggestion}` : ""
      }`;
      await navigator.clipboard.writeText(textToCopy);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      // 忽略剪贴板写入失败
    }
  };

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
          title={copied ? "已复制" : "复制错误信息"}
          aria-label={copied ? "已复制" : "复制错误信息"}
        >
          {copied ? <IconCheck /> : <IconCopy />}
          <span className="copy-label">{copied ? "已复制" : "复制"}</span>
        </button>
      </div>

      {info.suggestion && (
        <div className="chat-error-suggestion">
          <span className="suggestion-prefix">建议：</span>
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
            <span>{openRaw ? "收起详细错误信息" : "展开详细错误信息"}</span>
          </summary>
          <pre className="chat-error-pre">{info.rawError}</pre>
        </details>
      ) : (
        <div className="chat-error-message">{info.rawError}</div>
      )}
    </div>
  );
}
