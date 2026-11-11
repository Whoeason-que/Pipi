import { useState } from "react";
import {
  type ChatEntry,
  type Chip
} from "../chat-runtime";
import { IconCheck, IconChevronRight, IconCopy } from "../icons";
import { useCopyFeedback } from "../components/use-copy";
import { DiffView } from "../files/DiffView";
import { useI18n } from "../i18n";


export function formatClock(timestamp: number): string {
  const date = new Date(timestamp);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** 消息操作：复制正文（hover 出现的操作行里，成功后短暂显示对勾） */
export function CopyButton({ text }: { text: string }) {
  const { t } = useI18n();
  const { state, copy } = useCopyFeedback();
  const label = state === "copied" ? t("common.copied")
    : state === "failed" ? t("chat.copyFailed") : t("common.copy");
  return (
    <button
      type="button"
      className={`icon-btn copy-btn${state === "copied" ? " copied" : ""}${state === "failed" ? " copy-failed" : ""}`}
      title={label}
      aria-label={state === "idle" ? t("chat.copyMessage") : label}
      disabled={!text}
      onClick={() => void copy(text)}
    >
      {state === "copied" ? <IconCheck /> : <IconCopy />}
      {state === "failed" && <span className="copy-error-note" role="status">{label}</span>}
    </button>
  );
}

/**
 * 正文里的标签行：连续的工具调用 / thinking 压成一行（保持顺序、不合并）。
 * 悬浮 = 右栏临时预览，点击 = 固定（再点取消）；状态由标签自身承载：
 * 运行中脉冲、失败红色、成功低调。
 */
export function ChipRow({
  chips,
  activeId,
  pinnedId,
  onPreview,
  onPreviewEnd,
  onTogglePin,
}: {
  chips: Chip[];
  activeId: string | null;
  pinnedId: string | null;
  onPreview: (id: string) => void;
  onPreviewEnd: () => void;
  onTogglePin: (id: string) => void;
}) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const hidden = Math.max(0, chips.length - 16);
  const visibleChips = expanded ? chips : chips.slice(hidden);
  const hiddenFailures = chips.slice(0, hidden).filter(chip => chip.status === "error").length;
  return (
    <div className="row chip-line">
      <div className="row-inner">
        <div className="chip-row">
          {hidden > 0 && (
            <button type="button" className={`chip chip-expand${hiddenFailures ? " chip-error" : ""}`} aria-expanded={expanded}
              onClick={() => setExpanded(value => !value)}>
              {expanded ? t("chat.hideEarlierCalls") : hiddenFailures
                ? t("chat.showEarlierFailures", { count: hidden, failed: hiddenFailures })
                : t("chat.showEarlierCalls", { count: hidden })}
            </button>
          )}
          {visibleChips.map((chip) => {
            const classes = ["chip", `chip-${chip.status}`];
            if (chip.id === activeId) classes.push("active");
            if (chip.id === pinnedId) classes.push("pinned");
            return (
              <button
                key={chip.id}
                type="button"
                className={classes.join(" ")}
                onMouseEnter={() => onPreview(chip.id)}
                onMouseLeave={onPreviewEnd}
                onFocus={() => onPreview(chip.id)}
                onBlur={onPreviewEnd}
                onClick={() => onTogglePin(chip.id)}
                title={t("chat.chipTitle", {
                  name: chip.name,
                  status:
                    chip.status === "running"
                      ? t("common.running")
                      : chip.status === "error"
                        ? t("common.failed")
                        : t("chat.statusDone"),
                })}
                aria-expanded={chip.id === pinnedId}
              >
                {chip.status === "running" && <span className="chip-dot" aria-hidden="true" />}
                {chip.status === "error" && <span className="chip-x" aria-hidden="true">✕</span>}
                <span className="chip-name">{chip.name}</span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function ToolResultCard({ entry }: { entry: ChatEntry }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(!entry.text.includes("\n") || Boolean(entry.isError) || Boolean(entry.toolRunning));
  const toolName = entry.toolName ?? "tool";
  const running = Boolean(entry.toolRunning);
  const failed = Boolean(entry.isError);

  let commandStr: string | null = null;
  let pathStr: string | null = null;
  if (entry.toolArgs && typeof entry.toolArgs === "object") {
    const args = entry.toolArgs as Record<string, unknown>;
    if (typeof args.command === "string") commandStr = args.command;
    if (typeof args.path === "string") pathStr = args.path;
  }

  const details = entry.toolDetails as { diff?: string } | undefined;
  const hasDiff = typeof details?.diff === "string" && details.diff.trim().length > 0;
  const argsText = entry.toolArgs != null ? JSON.stringify(entry.toolArgs, null, 2) : null;
  const summary = commandStr ?? pathStr;

  return (
    <div className={`tool${running ? " tool-running" : failed ? " tool-error" : ""}`}>
      <button
        type="button"
        className="tool-head"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <span className="tool-name">{toolName}</span>
        {summary && (
          <span className="truncate tool-cmd" title={summary}>
            {summary}
          </span>
        )}
        <span className="caption tool-meta">
          {running ? (
            <span className="run">● {t("common.running")}</span>
          ) : failed ? (
            <span className="fail">✕ {t("common.failed")}</span>
          ) : (
            <span className="ok">✓ {t("chat.statusDone")}</span>
          )}
          <IconChevronRight expanded={open} />
        </span>
      </button>

      {open && (
        <div className="tool-body">
          {argsText && (
            <div className="tpane">
              <div className="caption plabel">{t("inspector.args")}</div>
              <pre className="code">{argsText}</pre>
            </div>
          )}
          <div className={`tpane${argsText ? "" : " full"}`}>
            <div className="caption plabel">{t("inspector.output")}</div>
            {hasDiff ? (
              // 和 Files 面板同一份 patch，用同一个渲染器：高亮、行号与逐词差异
              // 都由 @pierre/diffs 提供，不再自己按行首字符上色。
              <div className="tool-diff">
                <DiffView patch={details!.diff!} hideSummary />
              </div>
            ) : (
              <pre className="code">{toolOutputBody(entry) || (running ? t("inspector.runningOutput") : t("inspector.noOutput"))}</pre>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/** 去除 runtime 前缀（⚙ 工具名 / ✕）后的工具输出正文。 */
function toolOutputBody(entry: ChatEntry): string {
  const text = entry.text ?? "";
  const okPrefix = `⚙ ${entry.toolName ?? "tool"}`;
  if (text.startsWith(okPrefix)) {
    return text.slice(okPrefix.length).replace(/^\n/, "");
  }
  if (text.startsWith("✕ ")) return text.slice(2);
  return text;
}

/**
 * 右栏「调用详情」面板：承载被选中标签的完整内容。
 * - 工具标签：该次调用的可折叠卡（参数 / 输出 / diff，上下排列）
 * - thinking 标签：该段思考正文
 * - 固定（pinned）时显示取消固定按钮；悬浮预览时标注「预览」
 */
export function ChipDetailPane({
  chip,
  entries,
  mode,
  canUnpin,
  onUnpin,
}: {
  chip: Chip;
  entries: ChatEntry[];
  mode: "preview" | "pinned";
  canUnpin: boolean;
  onUnpin: () => void;
}) {
  const { t } = useI18n();
  const entry = entries.find((candidate) => candidate.key === chip.entryKey);
  return (
    <div className="detail-pane">
      <div className="tool-detail-head">
        <span className={`detail-name chip-${chip.status}`}>{chip.name}</span>
        <span className="caption detail-mode">{mode === "preview" ? t("inspector.preview") : t("inspector.pinned")}</span>
        <span className="spacer" />
        {canUnpin && (
          <button
            type="button"
            className="icon-btn detail-unpin"
            onClick={onUnpin}
            title={t("inspector.unpin")}
            aria-label={t("inspector.unpin")}
          >
            ×
          </button>
        )}
      </div>
      <div className="detail-body">
        {chip.kind === "thinking" ? (
          <pre className="thinking-body">{chip.thinking ?? entry?.thinking ?? ""}</pre>
        ) : entry ? (
          <ToolResultCard entry={entry} />
        ) : (
          <div className="empty-state detail-empty">{t("inspector.entryGone")}</div>
        )}
      </div>
    </div>
  );
}
