import { memo, useMemo, useRef, useState } from "react";
import { useI18n } from "../i18n";
import { IconArrowDown, IconArrowUp } from "../icons";
import { parsePatch, type DiffRow } from "./diff";

const SIGN: Record<DiffRow["kind"], string> = {
  add: "+",
  del: "−",
  context: " ",
  hunk: "",
  meta: "",
  note: "",
};

/** 统一 diff 表格：VS Code 风格双行号、hunk 分节与跳转导航、增删高亮。 */
export const DiffView = memo(function DiffView({
  patch,
  emptyText,
  hideFileHeaders = false,
  hideHunkHeaders = false,
}: {
  patch: string;
  emptyText?: string;
  /** 工作区 Git 预览已用标题栏展示路径，省略 diff --git/index/---/+++ 前缀行。 */
  hideFileHeaders?: boolean;
  /** 工作区主区只保留代码差异，省略 @@ hunk 行与跳转导航。 */
  hideHunkHeaders?: boolean;
}) {
  const { t } = useI18n();
  const parsed = useMemo(() => parsePatch(patch), [patch]);
  const rows = useMemo(
    () => parsed.rows.filter((row) => {
      if (row.kind === "hunk" && hideHunkHeaders) return false;
      if (hideFileHeaders && row.kind === "meta" && /^(diff --git |index |--- |\+\+\+ )/.test(row.text)) return false;
      return true;
    }),
    [parsed.rows, hideFileHeaders, hideHunkHeaders],
  );
  const [activeHunk, setActiveHunk] = useState(0);
  const [wrap, setWrap] = useState(false);
  const hunkRefs = useRef<Array<HTMLDivElement | null>>([]);

  const hunkIndices = useMemo(() => {
    const indices: number[] = [];
    parsed.rows.forEach((row, idx) => {
      if (row.kind === "hunk") indices.push(idx);
    });
    return indices;
  }, [parsed.rows]);
  const currentHunk = Math.min(activeHunk, Math.max(0, hunkIndices.length - 1));

  const jumpToHunk = (index: number) => {
    if (!hunkIndices.length) return;
    const bounded = Math.max(0, Math.min(index, hunkIndices.length - 1));
    setActiveHunk(bounded);
    const element = hunkRefs.current[bounded];
    element?.scrollIntoView({ behavior: "smooth", block: "center" });
  };

  if (!rows.length) return <p className="file-notice">{emptyText ?? t("files.noTextDiff")}</p>;

  let hunkCounter = 0;

  return (
    <div className={`diff-view vscode-diff${wrap ? " wrap" : ""}`}>
      <div className="diff-summary">
        <span className="diff-stat added">+{parsed.added}</span>
        <span className="diff-stat removed">−{parsed.removed}</span>
        <span className="diff-stat muted">{t("files.hunkCount", { count: parsed.hunks })}</span>
        <div className="diff-hunk-nav" role="group" aria-label={t("files.diffTools")}>
          <button type="button" className="diff-wrap-toggle" aria-pressed={wrap} onClick={() => setWrap(value => !value)} title={t("files.toggleWrap")}>{t("files.wordWrap")}</button>
          {!hideHunkHeaders && hunkIndices.length > 1 && <>
            <span className="diff-hunk-pos">
              {currentHunk + 1} / {hunkIndices.length}
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
              disabled={currentHunk >= hunkIndices.length - 1}
              onClick={() => jumpToHunk(currentHunk + 1)}
            >
              <IconArrowDown />
            </button>
          </>}
        </div>
      </div>
      <div className="diff-lines">
        <div className="diff-table" role="table" aria-label={t("files.textDiff")}>
          {rows.map((row, index) => {
            if (row.kind === "hunk") {
              const hIdx = hunkCounter++;
              return (
                <div
                  className="diff-row hunk"
                  role="row"
                  key={index}
                  ref={(el) => {
                    hunkRefs.current[hIdx] = el;
                  }}
                >
                  <span className="diff-code hunk-label" role="cell">
                    {row.text || " "}
                  </span>
                </div>
              );
            }
            if (row.kind === "meta") {
              return (
                <div className="diff-row meta" role="row" key={index}>
                  <span className="diff-code" role="cell">
                    {row.text || " "}
                  </span>
                </div>
              );
            }
            return (
              <div className={`diff-row ${row.kind}`} role="row" key={index}>
                <span className="diff-gutter old-line" role="cell" aria-hidden="true">
                  {row.oldLine ?? ""}
                </span>
                <span className="diff-gutter new-line" role="cell" aria-hidden="true">
                  {row.newLine ?? ""}
                </span>
                <span className="diff-sign" aria-hidden="true">
                  {row.kind === "note" ? "\\" : SIGN[row.kind]}
                </span>
                <span className="diff-code" role="cell">
                  {row.text || " "}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
});
