import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { assistantFooter, type ChatBlock } from "../chat-runtime";
import { Markdown } from "../Markdown";
import { ChipRow, CopyButton, formatClock } from "./ToolDetails";
import { IconFork } from "../icons";
import CompactionCard from "./CompactionCard";
import BackgroundResultCard from "./BackgroundResultCard";
import { ChatErrorCard } from "./ChatErrorCard";
import { extractAssistantContentAndError } from "./chat-errors";
import { useI18n } from "../i18n";
import type { ImageAgent } from "../files/MarkdownImage";

/** 未测量前的行高估计；真实高度由 ResizeObserver 测量后覆盖。 */
function estimateBlockSize(block: ChatBlock | undefined): number {
  if (!block) return 120;
  if (block.kind === "chips") return 48;
  if (block.kind === "system") return 88;
  if (block.kind === "user") return block.entry.images?.length ? 180 : 112;
  return 120;
}

export default function ChatTimeline({
  blocks,
  connecting,
  agent,
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
  scrollRef,
  pinnedToBottom,
}: {
  blocks: ChatBlock[];
  connecting: boolean;
  agent: ImageAgent;
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
  /** 聊天滚动容器（`.log`）；虚拟列表挂在它下面。 */
  scrollRef: RefObject<HTMLDivElement | null>;
  /** 用户仍停留在底部：新内容到达时跟随。 */
  pinnedToBottom: boolean;
}) {
  const { locale, t } = useI18n();
  const virtualizer = useVirtualizer({
    count: blocks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => estimateBlockSize(blocks[index]),
    getItemKey: (index) => blocks[index]?.key ?? index,
    overscan: 6,
  });
  const totalSize = virtualizer.getTotalSize();
  const findIndex = activeFindKey
    ? blocks.findIndex((block) => block.key === activeFindKey)
    : -1;
  const lastFindKey = useRef<string | null>(null);
  const pinTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 搜索命中可能在虚拟列表外：按块索引滚动，不再依赖 DOM 里已有目标节点。
  useEffect(() => {
    if (!activeFindKey) {
      lastFindKey.current = null;
      return;
    }
    if (lastFindKey.current === activeFindKey || findIndex < 0) return;
    lastFindKey.current = activeFindKey;
    virtualizer.scrollToIndex(findIndex, { align: "center" });
  }, [activeFindKey, findIndex, virtualizer]);

  // 跟随流式输出：用宏任务而不是 rAF —— 窗口不可见/失焦时 rAF 会被暂停，
  // 而贴底是数据一致性行为，不该依赖下一帧。交给虚拟器自己的 scrollToEnd，
  // 它会同时更新内部 range 与滚动命令状态；测量后的总高度变化会再次触发。
  useLayoutEffect(() => {
    if (!pinnedToBottom) {
      if (pinTimer.current !== null) {
        clearTimeout(pinTimer.current);
        pinTimer.current = null;
      }
      return;
    }
    if (pinTimer.current !== null) return;
    pinTimer.current = setTimeout(() => {
      pinTimer.current = null;
      virtualizer.scrollToEnd({ behavior: "auto" });
    }, 0);
  }, [pinnedToBottom, totalSize, blocks, connecting, virtualizer]);
  useEffect(() => () => {
    if (pinTimer.current !== null) clearTimeout(pinTimer.current);
  }, []);

  const renderBlock = (block: ChatBlock) => {
    if (block.kind === "system") {
      const entry = block.entry;
      // 重试提示是单行通知：不折叠，也没有摘要正文
      if (entry.kind === "retry") {
        return (
          <div className="row system-row">
            <div className="row-inner">
              <div className="retry-note">{entry.text}</div>
            </div>
          </div>
        );
      }
      if (entry.kind === "background" && entry.background) {
        return (
          <div className="row system-row">
            <div className="row-inner">
              <BackgroundResultCard result={entry.background} />
            </div>
          </div>
        );
      }
      return (
        <div className="row system-row">
          <div className="row-inner">
            <CompactionCard entry={entry} agent={agent} />
          </div>
        </div>
      );
    }
    if (block.kind === "chips") {
      return (
        <ChipRow
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
        <div data-entry-key={block.key} className={`row user-row${findCls}`}>
          <div className="row-inner">
            <div className="user-bubble">
              {entry.text && <div className="user-text">{entry.text}</div>}
              {!!entry.images?.length && <div className="user-images">{entry.images.map((image, index) => ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(image.mimeType) ? <img key={index} src={`data:${image.mimeType};base64,${image.data}`} alt={t("chat.attachedImage")} /> : null)}</div>}
            </div>
            <div className="row-meta">
              {entry.timestamp ? (
                <span className="caption time">{formatClock(entry.timestamp)}</span>
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
      <div data-entry-key={block.key} className={`row${findCls}${errorCls}`}>
        <div className="row-inner">
          {cleanText ? (
            <div className="content">
              <Markdown text={cleanText} imageAgent={agent} />
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
              {footer && <span className="hint truncate meta-info">{footer}</span>}
              <span className="spacer" />
              {entry.timestamp ? (
                <span className="caption time">{formatClock(entry.timestamp)}</span>
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
  };

  return (
    <>
      <div className="timeline-virtual" style={{ height: totalSize }}>
        {virtualizer.getVirtualItems().map((item) => {
          const block = blocks[item.index];
          if (!block) return null;
          return (
            <div
              key={item.key}
              ref={virtualizer.measureElement}
              data-index={item.index}
              className="timeline-item"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              {renderBlock(block)}
            </div>
          );
        })}
      </div>
      {connecting && (
        <div className="row connecting-row">
          <div className="row-inner" role="status">
            <span className="connecting-dot" aria-hidden="true" />
            <span>{t("chat.connecting")}</span>
          </div>
        </div>
      )}
    </>
  );
}
