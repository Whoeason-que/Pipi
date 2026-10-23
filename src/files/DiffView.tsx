import { memo, useMemo, useRef, useState } from "react";
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
  emptyText = "此文件没有文本差异。",
}: {
  patch: string;
  emptyText?: string;
}) {
  const parsed = useMemo(() => parsePatch(patch), [patch]);
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

  if (!parsed.rows.length) return <p className="file-notice">{emptyText}</p>;

  let hunkCounter = 0;

  return (
    <div className={`diff-view vscode-diff${wrap ? " wrap" : ""}`}>
      <div className="diff-summary">
        <span className="diff-stat added">+{parsed.added}</span>
        <span className="diff-stat removed">−{parsed.removed}</span>
        <span className="diff-stat muted">{parsed.hunks} 个差异块</span>
        <div className="diff-hunk-nav" role="group" aria-label="差异视图工具">
          <button type="button" className="diff-wrap-toggle" aria-pressed={wrap} onClick={() => setWrap(value => !value)} title="切换自动换行">自动换行</button>
          {hunkIndices.length > 1 && <>
            <span className="diff-hunk-pos">
              {currentHunk + 1} / {hunkIndices.length}
            </span>
            <button
              type="button"
              className="icon-btn small"
              title="上一处更改"
              aria-label="上一处更改"
              disabled={currentHunk <= 0}
              onClick={() => jumpToHunk(currentHunk - 1)}
            >
              <IconArrowUp />
            </button>
            <button
              type="button"
              className="icon-btn small"
              title="下一处更改"
              aria-label="下一处更改"
              disabled={currentHunk >= hunkIndices.length - 1}
              onClick={() => jumpToHunk(currentHunk + 1)}
            >
              <IconArrowDown />
            </button>
          </>}
        </div>
      </div>
      <div className="diff-lines">
        <div className="diff-table" role="table" aria-label="文本差异">
          {parsed.rows.map((row, index) => {
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
