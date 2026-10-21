import type { ChatEntry } from "../chat-runtime";
import type { FileReference, ToolFile, WorkspaceFile } from "./types";

/** 这些是工具操作记录，不等同于 Git diff 或会话开始时的工作区快照。 */
export function toolFiles(entries: ChatEntry[]): ToolFile[] {
  const files = new Map<string, ToolFile>();
  for (const entry of entries) {
    if (entry.role !== "toolResult" || !["read", "write", "edit"].includes(entry.toolName ?? "")) continue;
    const path = (entry.toolArgs as { path?: unknown } | undefined)?.path;
    if (typeof path !== "string" || !path) continue;
    const item = files.get(path) ?? { path, calls: 0, errors: 0, operations: [] };
    item.calls++;
    if (entry.isError) item.errors++;
    const details = entry.toolDetails as { diff?: unknown } | undefined;
    item.operations.push({ id: entry.key, name: entry.toolName!, patch: typeof details?.diff === "string" ? details.diff : null, error: !!entry.isError });
    files.set(path, item);
  }
  return [...files.values()].reverse();
}

export function codeFence(text: string, language = ""): string {
  const longest = (text.match(/`+/g) ?? []).reduce((max, run) => Math.max(max, run.length), 2);
  const fence = "`".repeat(longest + 1);
  return `${fence}${language}\n${text}\n${fence}`;
}

export function makeReference(file: WorkspaceFile, lines: [number, number] | null, comment: string): FileReference {
  const reference: FileReference = { workspace: file.workspace, path: file.path, version: file.version, comment: comment.trim() || undefined };
  if (lines && file.content !== null && !file.truncated) {
    const source = file.content.split("\n");
    const start = Math.max(1, Math.min(...lines));
    const end = Math.min(source.length, Math.max(...lines));
    if (end < start) throw new Error("请选择有效的行范围");
    const excerpt = source.slice(start - 1, end).join("\n");
    if (excerpt.length > 16_000) throw new Error("选区过大，请缩小到 16000 字符以内");
    Object.assign(reference, { startLine: start, endLine: end, excerpt });
  }
  return reference;
}

/** 用普通文本保存引用，复用既有草稿/steering/会话格式，不引入私有附件存储。 */
export function formatReference(reference: FileReference): string {
  const locator = reference.startLine === undefined ? "" : ` · L${reference.startLine}–${reference.endLine}`;
  const identity = JSON.stringify({ workspace: reference.workspace, path: reference.path, sha256: reference.version || null });
  return `文件引用${locator}\n${identity}${reference.excerpt === undefined ? "" : `\n${codeFence(reference.excerpt)}`}${reference.comment ? `\n意见：${reference.comment}` : ""}`;
}

export function parentPath(path: string): string { return path.split("/").slice(0, -1).join("/"); }

/** Markdown 的本地链接相对当前文件解析；绝对路径仍由宿主校验，网络链接不在这里读取。 */
export function linkedPath(file: string, href: string): string | null {
  if (!href || href.startsWith("#") || /^[a-z][a-z\d+.-]*:/i.test(href) || href.startsWith("//")) return null;
  let decoded: string;
  try { decoded = decodeURIComponent(href.split(/[?#]/)[0]); } catch { return null; }
  if (decoded.startsWith("/")) return decoded;
  const parts: string[] = parentPath(file).split("/").filter(Boolean);
  for (const part of decoded.split("/")) {
    if (part === "..") { if (!parts.length) return null; parts.pop(); }
    else if (part && part !== ".") parts.push(part);
  }
  return parts.join("/");
}

export function languageFor(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return ({ ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript", rs: "rust", py: "python", json: "json", css: "css", sh: "bash", toml: "ini", yml: "yaml", yaml: "yaml", html: "xml", svg: "xml", md: "markdown", sql: "sql", c: "c", cpp: "cpp", go: "go" } as Record<string, string>)[ext] ?? "text";
}
