import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useCopyFeedback } from "../components/use-copy";
import { IconCopy, IconFile, IconGit } from "../icons";
import { DiffView } from "./DiffView";
import { FileContent } from "./FileContent";
import { FileGlyph } from "./FileGlyph";
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
  emptyTitle?: string;
  emptyHint?: ReactNode;
}

/** File、Git 和会话操作记录共用渲染；不拥有文件读取、工作区或会话身份。 */
export function FilePreview({ path, source, loading = false, error = "", tabs, onOpenFile, onReference,
  referenceOptions, referenceLabel = "加入输入框", referenceDisabled = false, scroll = 0, onScroll,
  emptyTitle = "选择文件开始预览", emptyHint = "在左侧文件树中选择文件；Git 页可查看与 HEAD 的逐行差异。" }: Props) {
  const [lines, setLines] = useState<[number, number] | null>(null);
  const [comment, setComment] = useState("");
  const [feedback, setFeedback] = useState("");
  const clipboard = useCopyFeedback();
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
  const closeTabs = <div className="file-preview-tabs-wrap">{tabs}</div>;

  return <section className="file-preview" aria-label="文件预览" aria-busy={loading}>
    {closeTabs}
    {path ? <>
      <div className="file-preview-title">
        <FileGlyph path={path} />
        <div className="file-preview-identity">
          <span title={path}>{path.split("/").pop()}</span>
          <small title={path}>{path}</small>
        </div>
        {source?.type === "file" && <span className="file-preview-chip" title={`内容版本 ${file?.version || "未知"}`}>
          {formatSize(source.file.size)}{source.file.version ? ` · ${source.file.version.slice(0, 8)}` : ""}
        </span>}
        {source?.type === "git" && <span className="file-preview-chip git" title={`比较基准 ${source.diff.base}`}>
          <IconGit /> {source.diff.base}
        </span>}
        {source?.type === "tool" && <span className={`file-preview-chip${source.error ? " danger" : ""}`}>{source.name}{source.error ? " 失败" : ""}</span>}
        <button type="button" className="icon-btn file-copy-path" title={clipboard.label === "复制" ? "复制文件路径" : clipboard.label}
          aria-label="复制文件路径" onClick={() => void clipboard.copy(path)}><IconCopy /></button>
        {source && source.type !== "file" && onOpenFile && <button type="button" className="btn small ghost" onClick={() => onOpenFile(path)}>在 File 打开</button>}
      </div>
      <div className="file-preview-scroll" ref={scroller} onScroll={event => { if (!loading && source) onScroll?.(event.currentTarget.scrollTop); }}>
        {error && source && <p className="file-error" role="status">刷新失败，显示上次内容：{error}</p>}
        {loading ? <div className="file-state" role="status"><span className="file-state-spinner" />正在打开文件…</div>
          : error && !source ? <p className="file-error" role="alert">{error}</p>
          : source?.type === "file" ? <FileContent key={identity} file={source.file} lines={lines} onLines={setLines} onOpen={onOpenFile ?? (() => {})} />
          : source?.type === "git" ? <>
            <div className="file-base-note">
              <span>比较基准</span>
              <code>{source.diff.base}</code>
              {source.diff.truncated && <span className="tag warn">内容已截断</span>}
            </div>
            <DiffView key={source.diff.path} patch={source.diff.patch} />
          </>
          : source?.type === "tool" ? <>
            <p className="file-notice">本次 {source.name} 工具操作记录{source.error ? "（失败）" : ""}，与磁盘现状分别展示。</p>
            <DiffView patch={source.patch} emptyText="这次工具调用没有可展示的文本差异。" />
          </> : null}
      </div>
      {file && onReference && <div className="file-reference">
        <div className="file-reference-head">
          <span>{lines ? <><b>L{Math.min(...lines)}–{Math.max(...lines)}</b> 选区</> : "引用当前文件"}</span>
          {lines && <button type="button" className="btn small ghost" onClick={() => setLines(null)}>清除选区</button>}
        </div>
        {referenceOptions}
        <textarea className="text-input" aria-label="文件引用意见" placeholder="针对文件或选区补充意见（可选）" value={comment} onChange={e => setComment(e.target.value)} rows={2} />
        <div className="file-reference-actions">
          <button type="button" className="btn primary" onClick={reference} disabled={referenceDisabled}>{referenceLabel}</button>
          <p role="status">{feedback}</p>
        </div>
      </div>}
    </> : <div className="file-preview-empty">
      <span className="file-empty-icon"><IconFile /></span>
      <strong>{emptyTitle}</strong>
      <p>{emptyHint}</p>
    </div>}
  </section>;
}

export function formatSize(bytes: number): string { return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MiB` : bytes >= 1024 ? `${(bytes / 1024).toFixed(1)} KiB` : `${bytes} B`; }
