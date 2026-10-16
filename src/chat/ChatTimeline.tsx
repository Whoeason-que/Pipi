import { assistantFooter, type ChatBlock } from "../chat-runtime";
import { Markdown } from "../Markdown";
import { ChipRow, CopyButton, formatClock } from "./ToolDetails";
import { IconFork } from "../icons";
import { ChatErrorCard } from "./ChatErrorCard";
import { extractAssistantContentAndError } from "./chat-errors";
import { useI18n } from "../i18n";

export default function ChatTimeline({
  blocks,
  activeChipId,
  pinnedChip,
  previewDetail,
  clearPreviewDetail,
  togglePinnedDetail,
  findHits,
  activeFindKey,
  previewFindKey,
  pinnedFindKey,
  onFork,
  forkDisabled,
}: {
  blocks: ChatBlock[];
  activeChipId: string | null;
  pinnedChip: string | null;
  findHits: string[];
  activeFindKey: string | null;
  previewFindKey?: string | null;
  pinnedFindKey?: string | null;
  previewDetail: (id: string) => void;
  clearPreviewDetail: () => void;
  togglePinnedDetail: (id: string) => void;
  onFork?: () => void;
  forkDisabled?: boolean;
}) {
  const { locale, t } = useI18n();
  return <>
          {blocks.map((block) => {
            if (block.kind === "system") {
              const entry = block.entry;
              // 重试提示是单行通知：不折叠，也没有摘要正文
              if (entry.kind === "retry") {
                return (
                  <div key={entry.key} className="row system-row">
                    <div className="row-inner">
                      <div className="retry-note">{entry.text}</div>
                    </div>
                  </div>
                );
              }
              return (
                <div key={entry.key} className="row system-row">
                  <div className="row-inner">
                    <details className="compaction-fold" open={!entry.summary}>
                      <summary>{entry.text}</summary>
                      {entry.summary ? <Markdown text={entry.summary} /> : null}
                    </details>
                  </div>
                </div>
              );
            }
            if (block.kind === "chips") {
              return (
                <ChipRow
                  key={block.key}
                  chips={block.chips}
                  activeId={activeChipId}
                  pinnedId={pinnedChip}
                  onPreview={previewDetail}
                  onPreviewEnd={clearPreviewDetail}
                  onTogglePin={togglePinnedDetail}
                />
              );
            }
            const entry = block.entry;
            if (block.kind === "user") {
              const isHit = findHits.includes(block.key);
              const isCurrent = activeFindKey === block.key;
              const isPreview = previewFindKey === block.key;
              const isPinned = pinnedFindKey === block.key;
              const findCls = isHit ? ` find-hit${isCurrent ? " find-current" : ""}${isPreview ? " find-preview" : ""}${isPinned ? " find-pinned" : ""}` : "";
              return (
                <div key={entry.key} data-entry-key={block.key} className={`row user-row${findCls}`}>
                  <div className="row-inner">
                    <div className="user-bubble">
                      <div className="user-text">{entry.text}</div>
                    </div>
                    <div className="row-meta">
                      {entry.timestamp ? (
                        <span className="time">{formatClock(entry.timestamp)}</span>
                      ) : null}
                      <CopyButton text={entry.text} />
                    </div>
                  </div>
                </div>
              );
            }
            // 正文块：assistant 文本（thinking 与工具调用已由标签行承载）
            const { cleanText, errorInfo } = extractAssistantContentAndError(entry, locale);
            const footer = !entry.streaming && !errorInfo && cleanText
              ? assistantFooter({
                  role: "assistant",
                  content: cleanText,
                  usage: entry.usage,
                  durationMs: entry.durationMs,
                })
              : null;
            const isHit = findHits.includes(block.key);
            const isCurrent = activeFindKey === block.key;
            const isPreview = previewFindKey === block.key;
            const isPinned = pinnedFindKey === block.key;
            const findCls = isHit ? ` find-hit${isCurrent ? " find-current" : ""}${isPreview ? " find-preview" : ""}${isPinned ? " find-pinned" : ""}` : "";
            const errorCls = errorInfo ? ` row-has-error row-error-${errorInfo.category}` : "";
            return (
              <div key={entry.key} data-entry-key={block.key} className={`row${findCls}${errorCls}`}>
                <div className="row-inner">
                  {cleanText ? (
                    <div className="content">
                      <Markdown text={cleanText} />
                      {entry.streaming && <span className="cursor" aria-hidden="true" />}
                    </div>
                  ) : entry.streaming ? (
                    <div className="content">
                      <span className="cursor" aria-hidden="true" />
                    </div>
                  ) : null}

                  {errorInfo && <ChatErrorCard info={errorInfo} />}

                  {!entry.streaming && (
                    // 统计信息与时间、按钮同一行；时间与按钮靠右（hover 时淡入）
                    <div className="row-meta">
                      {footer && <span className="hint meta-info">{footer}</span>}
                      <span className="spacer" />
                      {entry.timestamp ? (
                        <span className="time">{formatClock(entry.timestamp)}</span>
                      ) : null}
                      {cleanText ? <CopyButton text={cleanText} /> : null}
                      {onFork ? (
                        <button
                          type="button"
                          className="icon-btn fork-btn"
                          onClick={onFork}
                          disabled={forkDisabled}
                          title={forkDisabled ? t("chat.cannotFork") : t("chat.fork")}
                          aria-label={t("chat.fork")}
                        >
                          <IconFork />
                        </button>
                      ) : null}
                    </div>
                  )}
                </div>
              </div>
            );
          })}
  </>;
}
