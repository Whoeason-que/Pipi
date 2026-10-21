import { memo, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { FileContent } from "./FileContent";
import { formatReference, makeReference } from "./model";
import type { WorkspaceDiff, WorkspaceFile } from "./types";

export type PreviewSource = { type: "file"; file: WorkspaceFile }
  | { type: "git"; diff: WorkspaceDiff }
  | { type: "tool"; id: string; path: string; name: string; patch: string; error: boolean };

interface Props {
  path: string | null;
  source: PreviewSource | null;
  loading?: boolean;
  error?: string;
  tabs?: ReactNode;
  onOpenFile?: (path: string) => void;
  onReference?: (text: string) => void;
  referenceOptions?: ReactNode;
  referenceLabel?: string;
  referenceDisabled?: boolean;
  scroll?: number;
  onScroll?: (top: number) => void;
}

/** File、Git 和会话操作记录共用渲染；不拥有文件读取、工作区或会话身份。 */
export function FilePreview({ path, source, loading = false, error = "", tabs, onOpenFile, onReference,
  referenceOptions, referenceLabel = "加入输入框", referenceDisabled = false, scroll = 0, onScroll }: Props) {
  const [lines, setLines] = useState<[number, number] | null>(null);
  const [comment, setComment] = useState("");
  const [feedback, setFeedback] = useState("");
  const scroller = useRef<HTMLDivElement>(null);
  const file = source?.type === "file" ? source.file : null;
  const identity = file ? `${path}:${file.version}` : source?.type === "tool" ? source.id : path;
  useEffect(() => { setLines(null); setComment(""); setFeedback(""); }, [identity]);
  useLayoutEffect(() => { if (!loading && source && scroller.current) scroller.current.scrollTop = scroll; }, [path, loading, identity]);
  const reference = () => {
    if (!file || !onReference) return;
    try { onReference(formatReference(makeReference(file, lines, comment))); setFeedback("引用已加入目标草稿。"); setComment(""); }
    catch (error) { setFeedback(String(error)); }
  };
  return <section className="file-preview" aria-label="文件预览">
    {tabs}
    {path ? <>
      <div className="file-preview-title"><span title={path}>{path}</span>
        {source && source.type !== "file" && onOpenFile && <button type="button" onClick={() => onOpenFile(path)}>在 File 打开</button>}
        {file && <small>{formatSize(file.size)}{file.version ? ` · ${file.version.slice(0, 8)}` : ""}</small>}
      </div>
      <div className="file-preview-scroll" ref={scroller} onScroll={event => { if (!loading && source) onScroll?.(event.currentTarget.scrollTop); }}>
        {loading ? <p className="file-notice" role="status">正在打开文件…</p> : error ? <p className="file-error" role="alert">{error}</p>
          : source?.type === "file" ? <FileContent key={identity} file={source.file} lines={lines} onLines={setLines} onOpen={onOpenFile ?? (() => {})} />
          : source?.type === "git" ? <><p className="file-notice">比较基准：{source.diff.base}{source.diff.truncated ? " · 内容已截断" : ""}</p><Patch text={source.diff.patch || "此文件没有文本差异。"} /></>
          : source?.type === "tool" ? <><p className="file-notice">本次 {source.name} 工具操作记录{source.error ? "（失败）" : ""}，与磁盘现状分别展示。</p><Patch text={source.patch} /></> : null}
      </div>
      {file && onReference && <div className="file-reference">
        <div><span>{lines ? `已选 L${Math.min(...lines)}–${Math.max(...lines)}` : "引用当前文件"}</span>{lines && <button type="button" onClick={() => setLines(null)}>清除选区</button>}</div>
        {referenceOptions}
        <textarea aria-label="文件引用意见" placeholder="针对文件或选区补充意见（可选）" value={comment} onChange={e => setComment(e.target.value)} rows={2} />
        <button type="button" className="btn" onClick={reference} disabled={referenceDisabled}>{referenceLabel}</button><p role="status">{feedback}</p>
      </div>}
    </> : <div className="file-preview-empty"><strong>选择文件开始预览</strong><p>在左侧选择文件，或使用导航按钮展开文件列表。</p></div>}
  </section>;
}
export function formatSize(bytes: number): string { return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MiB` : bytes >= 1024 ? `${(bytes / 1024).toFixed(1)} KiB` : `${bytes} B`; }
const Patch = memo(function Patch({ text }: { text: string }) {
  return <pre className="file-patch">{text.split("\n").map((line, index) => <span key={index} className={line.startsWith("+") ? "added" : line.startsWith("-") ? "removed" : line.startsWith("@@") ? "hunk" : ""}>{line}{"\n"}</span>)}</pre>;
});
