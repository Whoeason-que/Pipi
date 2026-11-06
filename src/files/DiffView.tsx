import { memo, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { PatchDiff } from "@pierre/diffs/react";
import type { FileDiffOptions } from "@pierre/diffs";
import { IconArrowDown, IconArrowUp } from "../icons";
import { useI18n } from "../i18n";
import { parsePatch, normalizePatch, type ParsedDiff } from "./diff";

type DiffHostStyle = CSSProperties & Record<`--diffs-${string}`, string>;

function currentThemeMode(): "light" | "dark" {
  if (typeof document === "undefined") return "dark";
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

/** 每个 hunk 的第一条内容行号；用于在库的 shadow DOM 里定位 hunk。 */
function hunkAnchors(rows: ParsedDiff["rows"]): number[] {
  const anchors: number[] = [];
  let pending = false;
  for (const row of rows) {
    if (row.kind === "hunk") {
      pending = true;
      continue;
    }
    if (!pending) continue;
    if (row.kind === "add" || row.kind === "del" || row.kind === "context") {
      const line = row.newLine ?? row.oldLine;
      if (line !== null) anchors.push(line);
      pending = false;
    }
  }
  return anchors;
}

/** 跟随应用主题（html[data-theme]），Diff 的明暗语法配色与背景随之切换。 */
function useThemeMode(): "light" | "dark" {
  const [mode, setMode] = useState(currentThemeMode);
  useEffect(() => {
    const root = document.documentElement;
    const update = () => setMode(currentThemeMode());
    update();
    const observer = new MutationObserver(update);
    observer.observe(root, { attributes: true, attributeFilter: ["data-theme"] });
    return () => observer.disconnect();
  }, []);
  return mode;
}

/**
 * 统一 diff：统计与换行开关由外壳拥有，正文交给 @pierre/diffs 的 PatchDiff
 * （Shiki 语法高亮、逐词差异、行号与 shadow DOM 隔离）。patch 只有局部上下文，
 * 因此使用 simple 分隔，避免展示无法展开的 unmodified lines。
 */
export const DiffView = memo(function DiffView({
  patch,
  emptyText,
  hideFileHeaders = false,
}: {
  patch: string;
  emptyText?: string;
  /** 工作区 Git 预览已用标题栏展示路径，省略库自带的文件头。 */
  hideFileHeaders?: boolean;
}) {
  const { t } = useI18n();
  const normalized = useMemo(() => normalizePatch(patch), [patch]);
  const parsed = useMemo(() => parsePatch(normalized), [normalized]);
  const anchors = useMemo(() => hunkAnchors(parsed.rows), [parsed.rows]);
  const [wrap, setWrap] = useState(false);
  const [activeHunk, setActiveHunk] = useState(0);
  const hostRef = useRef<HTMLDivElement>(null);
  const themeMode = useThemeMode();
  const currentHunk = Math.min(activeHunk, Math.max(0, anchors.length - 1));

  useEffect(() => {
    setActiveHunk(0);
  }, [normalized]);

  const jumpToHunk = (index: number) => {
    if (!anchors.length) return;
    const bounded = Math.max(0, Math.min(index, anchors.length - 1));
    setActiveHunk(bounded);
    const line = anchors[bounded];
    const host = hostRef.current?.querySelector("diffs-container");
    const target = host?.shadowRoot?.querySelector<HTMLElement>(`[data-line="${line}"]`);
    target?.scrollIntoView({ block: "center", behavior: "smooth" });
  };

  const options = useMemo<FileDiffOptions<undefined, undefined>>(() => ({
    diffStyle: "unified",
    diffIndicators: "classic",
    disableFileHeader: hideFileHeaders,
    hunkSeparators: "simple",
    overflow: wrap ? "wrap" : "scroll",
    lineDiffType: "word-alt",
    themeType: themeMode,
    preferredHighlighter: "shiki-wasm",
  }), [hideFileHeaders, themeMode, wrap]);

  const hostStyle = useMemo<DiffHostStyle>(() => ({
    "--diffs-bg": "var(--panel)",
    "--diffs-light-bg": "var(--panel)",
    "--diffs-dark-bg": "var(--panel)",
    "--diffs-fg": "var(--fg)",
    "--diffs-fg-number": "var(--faint)",
    "--diffs-fg-number-override": "var(--faint)",
    "--diffs-font-family": "var(--font-mono)",
    "--diffs-font-size": "12.5px",
    "--diffs-line-height": "22px",
    "--diffs-bg-context-override": "var(--panel)",
    "--diffs-bg-context-gutter-override": "var(--stripe)",
    "--diffs-bg-addition-override": "var(--ok-soft)",
    "--diffs-bg-deletion-override": "var(--danger-soft)",
    "--diffs-bg-addition-emphasis-override": "var(--ok-border)",
    "--diffs-bg-deletion-emphasis-override": "var(--danger-border)",
    "--diffs-bg-addition-number-override": "var(--ok-soft)",
    "--diffs-bg-deletion-number-override": "var(--danger-soft)",
    "--diffs-bg-separator-override": "var(--bg)",
  }), []);

  if (!parsed.rows.length) return <p className="file-notice">{emptyText ?? t("files.noTextDiff")}</p>;

  return (
    <div className="diff-view vscode-diff">
      <div className="diff-summary">
        <span className="diff-stat added">+{parsed.added}</span>
        <span className="diff-stat removed">−{parsed.removed}</span>
        <span className="diff-stat muted">{t("files.hunkCount", { count: parsed.hunks })}</span>
        <div className="diff-hunk-nav" role="group" aria-label={t("files.diffTools")}>
          <button type="button" className="diff-wrap-toggle" aria-pressed={wrap} onClick={() => setWrap(value => !value)} title={t("files.toggleWrap")}>{t("files.wordWrap")}</button>
          {anchors.length > 1 && <>
            <span className="diff-hunk-pos">
              {currentHunk + 1} / {anchors.length}
            </span>
            <button
              type="button"
              className="icon-btn small"
              title={t("files.previousChange")}
              aria-label={t("files.previousChange")}
              disabled={currentHunk <= 0}
              onClick={() => jumpToHunk(currentHunk - 1)}
            >
              <IconArrowUp />
            </button>
            <button
              type="button"
              className="icon-btn small"
              title={t("files.nextChange")}
              aria-label={t("files.nextChange")}
              disabled={currentHunk >= anchors.length - 1}
              onClick={() => jumpToHunk(currentHunk + 1)}
            >
              <IconArrowDown />
            </button>
          </>}
        </div>
      </div>
      <div className="diff-lines" ref={hostRef}>
        <PatchDiff
          patch={normalized}
          options={options}
          disableWorkerPool
          className="diff-host"
          style={hostStyle}
        />
      </div>
    </div>
  );
});
