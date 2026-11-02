// 统一 diff 的展示模型：Git 预览与会话工具差异共用。
// 只解析展示所需的位置信息（旧/新行号、增删统计），不改写补丁内容。

import { getDefaultLocale, getTranslation, type Locale } from "../i18n/core.ts";

export type DiffKind = "meta" | "hunk" | "context" | "add" | "del" | "note";

export interface DiffRow {
  kind: DiffKind;
  /** 行号只用于展示；上下文行同时有旧/新行号。 */
  oldLine: number | null;
  newLine: number | null;
  text: string;
}

export interface ParsedDiff {
  rows: DiffRow[];
  added: number;
  removed: number;
  hunks: number;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;
const FILE_HEADER = /^(diff --git |index |--- |\+\+\+ |new file mode |deleted file mode |old mode |new mode |similarity index |rename from |rename to |Binary files )/;

/** 逐行解析 unified diff。未知的 `@@` 形式按无行号块处理，仍保留内容。 */
export function parsePatch(patch: string): ParsedDiff {
  const rows: DiffRow[] = [];
  let oldLine: number | null = null;
  let newLine: number | null = null;
  let added = 0;
  let removed = 0;
  let hunks = 0;
  let inHunk = false;
  const lines = patch.split("\n");
  // 末尾换行会 split 出空串；它不代表文件里多了一行空行。
  if (lines.at(-1) === "") lines.pop();

  for (const text of lines) {
    const hunk = HUNK.exec(text);
    if (hunk) {
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[2]);
      hunks += 1;
      inHunk = true;
      rows.push({ kind: "hunk", oldLine: null, newLine: null, text });
      continue;
    }
    if (text.startsWith("@@")) {
      oldLine = null;
      newLine = null;
      hunks += 1;
      inHunk = true;
      rows.push({ kind: "hunk", oldLine: null, newLine: null, text });
      continue;
    }
    if (text.startsWith("diff --git ")) {
      inHunk = false;
      rows.push({ kind: "meta", oldLine: null, newLine: null, text });
      continue;
    }
    if (!inHunk && FILE_HEADER.test(text)) {
      rows.push({ kind: "meta", oldLine: null, newLine: null, text });
      continue;
    }
    if (text.startsWith("+")) {
      rows.push({ kind: "add", oldLine: null, newLine, text: text.slice(1) });
      added += 1;
      if (newLine !== null) newLine += 1;
      continue;
    }
    if (text.startsWith("-")) {
      rows.push({ kind: "del", oldLine, newLine: null, text: text.slice(1) });
      removed += 1;
      if (oldLine !== null) oldLine += 1;
      continue;
    }
    if (text.startsWith(" ")) {
      rows.push({ kind: "context", oldLine, newLine, text: text.slice(1) });
      if (oldLine !== null) oldLine += 1;
      if (newLine !== null) newLine += 1;
      continue;
    }
    if (text.startsWith("\\")) {
      rows.push({ kind: "note", oldLine: null, newLine: null, text });
      continue;
    }
    // 非标准行（例如工具预览的 `@@ new file @@` 后紧跟文本）按上下文展示，不猜测行号。
    rows.push({ kind: oldLine === null && newLine === null ? "meta" : "context", oldLine, newLine, text });
    if (oldLine !== null) oldLine += 1;
    if (newLine !== null) newLine += 1;
  }
  return { rows, added, removed, hunks };
}

/** Git 状态码的展示口径：首字符 = 暂存区，次字符 = 工作树。 */
export interface GitStatusMeta {
  code: string;
  label: string;
  tone: "added" | "modified" | "deleted" | "untracked" | "conflict" | "neutral";
  staged: boolean;
  unstaged: boolean;
}

export function gitStatusMeta(status: string, locale: Locale = getDefaultLocale()): GitStatusMeta {
  const code = status.slice(0, 2).padEnd(2, " ");
  const staged = code[0] !== " " && code[0] !== "?";
  const unstaged = code[1] !== " " && code[1] !== "?";
  const label = (key: string) => getTranslation(locale, `files.status.${key}`);
  if (code.includes("?")) return { code: "??", label: label("untracked"), tone: "untracked", staged: false, unstaged: true };
  if (code.includes("U") || code === "AA" || code === "DD") {
    return { code, label: label("conflict"), tone: "conflict", staged, unstaged };
  }
  if (code.includes("A")) return { code, label: label("added"), tone: "added", staged, unstaged };
  if (code.includes("D")) return { code, label: label("deleted"), tone: "deleted", staged, unstaged };
  if (code.includes("R")) return { code, label: label("renamed"), tone: "modified", staged, unstaged };
  if (code.includes("M") || code.includes("T")) return { code, label: label("modified"), tone: "modified", staged, unstaged };
  return { code, label: label("changed"), tone: "neutral", staged, unstaged };
}

/**
 * 改动摘要（侧栏与右栏共用一份口径）。
 *
 * 同时含暂存与工作区更改的文件（`MM`）计入暂存组 —— 与侧栏「更改」列表
 * 用 `!meta.staged` 过滤的规则一致，避免两处显示不同的未暂存数。
 */
export function summarizeChanges(files: ReadonlyArray<{ status: string }>) {
  let staged = 0;
  let unstaged = 0;
  let untracked = 0;
  for (const file of files) {
    const meta = gitStatusMeta(file.status);
    if (meta.tone === "untracked") untracked += 1;
    else {
      if (meta.staged) staged += 1;
      if (meta.unstaged && !meta.staged) unstaged += 1;
    }
  }
  return { staged, unstaged, untracked, total: files.length };
}
