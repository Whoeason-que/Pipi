import { useState } from "react";
import {
  type ChatEntry,
  type Chip
} from "../chat-runtime";
import { IconCheck, IconCopy } from "../icons";


export function formatClock(timestamp: number): string {
  const date = new Date(timestamp);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** 消息操作：复制正文（hover 出现的操作行里，成功后短暂显示对勾） */
export function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={`icon-btn copy-btn${copied ? " copied" : ""}`}
      title={copied ? "已复制" : "复制"}
      aria-label={copied ? "已复制" : "复制消息"}
      disabled={!text}
      onClick={() => {
        navigator.clipboard
          ?.writeText(text)
          .then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          })
          .catch(() => {
            // 剪贴板不可用时静默失败，不打断阅读
          });
      }}
    >
      {copied ? <IconCheck /> : <IconCopy />}
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
  return (
    <div className="row chip-line">
      <div className="row-inner">
        <div className="chip-row">
          {chips.map((chip) => {
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
                title={`${chip.name} · ${
                  chip.status === "running" ? "运行中" : chip.status === "error" ? "失败" : "已完成"
                }（悬浮预览 · 点击固定到右栏）`}
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
          <span className="tool-cmd" title={summary}>
            {summary}
          </span>
        )}
        <span className="tool-meta">
          {running ? (
            <span className="run">● 运行中</span>
          ) : failed ? (
            <span className="fail">✕ 失败</span>
          ) : (
            <span className="ok">✓ 完成</span>
          )}
          <span>{open ? "▾" : "▸"}</span>
        </span>
      </button>

      {open && (
        <div className="tool-body">
          {argsText && (
            <div className="tpane">
              <div className="plabel">参数</div>
              <pre className="code">{argsText}</pre>
            </div>
          )}
          <div className={`tpane${argsText ? "" : " full"}`}>
            <div className="plabel">输出</div>
            {hasDiff ? (
              <pre className="code">
                {details!.diff!.split("\n").map((line, index) => {
                  let lineClass = "ln-ctx";
                  if (line.startsWith("+++") || line.startsWith("---")) lineClass = "ln-ctx";
                  else if (line.startsWith("+")) lineClass = "ln-add";
                  else if (line.startsWith("-")) lineClass = "ln-del";
                  else if (line.startsWith("@@")) lineClass = "ln-hunk";
                  return (
                    <span key={index} className={lineClass}>
                      {line}
                    </span>
                  );
                })}
              </pre>
            ) : (
              <pre className="code">{toolOutputBody(entry) || (running ? "执行中…" : "（无输出）")}</pre>
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
  const entry = entries.find((candidate) => candidate.key === chip.entryKey);
  return (
    <div className="detail-pane">
      <div className="detail-head">
        <span className={`detail-name chip-${chip.status}`}>{chip.name}</span>
        <span className="detail-mode">{mode === "preview" ? "预览" : "已固定"}</span>
        <span className="spacer" />
        {canUnpin && (
          <button
            type="button"
            className="icon-btn detail-unpin"
            onClick={onUnpin}
            title="取消固定"
            aria-label="取消固定"
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
          <div className="detail-empty">该条目的记录已不在当前会话中。</div>
        )}
      </div>
    </div>
  );
}
