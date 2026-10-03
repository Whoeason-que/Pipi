import { assistantFooter, type ChatBlock } from "../chat-runtime";
import { Markdown } from "../Markdown";
import { ChipRow, CopyButton, formatClock } from "./ToolDetails";

export default function ChatTimeline({ blocks, activeChipId, pinnedChip, previewDetail, clearPreviewDetail, togglePinnedDetail }: {
  blocks: ChatBlock[]; activeChipId: string | null; pinnedChip: string | null;
  previewDetail: (id: string) => void; clearPreviewDetail: () => void; togglePinnedDetail: (id: string) => void;
}) {
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
              return (
                <div key={entry.key} className="row user-row">
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
            const footer = !entry.streaming && !entry.status
              ? assistantFooter({
                  role: "assistant",
                  content: entry.text,
                  usage: entry.usage,
                  durationMs: entry.durationMs,
                })
              : null;
            return (
              <div key={entry.key} className="row">
                <div className="row-inner">
                  <div className="content">
                    <Markdown text={entry.text} />
                    {entry.streaming && <span className="cursor" aria-hidden="true" />}
                  </div>
                  {!entry.streaming && (
                    // 统计信息与时间、按钮同一行；时间与按钮靠右（hover 时淡入）
                    <div className="row-meta">
                      {footer && <span className="hint meta-info">{footer}</span>}
                      <span className="spacer" />
                      {entry.timestamp ? (
                        <span className="time">{formatClock(entry.timestamp)}</span>
                      ) : null}
                      <CopyButton text={entry.text} />
                    </div>
                  )}
                </div>
              </div>
            );
          })}
  </>;
}
