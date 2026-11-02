import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useCopyFeedback } from "../components/use-copy";
import { useI18n } from "../i18n";
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
  referenceOptions, referenceLabel, referenceDisabled = false, scroll = 0, onScroll,
  emptyTitle, emptyHint }: Props) {
  const { t } = useI18n();
  const resolvedReferenceLabel = referenceLabel ?? t("files.addToComposer");
  const resolvedEmptyTitle = emptyTitle ?? t("files.selectFileTitle");
  const resolvedEmptyHint = emptyHint ?? t("files.selectFileHint");
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
    try { onReference(formatReference(makeReference(file, lines, comment))); setFeedback(t("files.referenceAdded")); setComment(""); }
    catch (error) { setFeedback(String(error)); }
  };
  const closeTabs = <div className="file-preview-tabs-wrap">{tabs}</div>;

  return <section className="file-preview" aria-label={t("files.filePreview")} aria-busy={loading}>
    {closeTabs}
    {path ? <>
      <div className="file-preview-title">
        <FileGlyph path={path} />
        <div className="file-preview-identity">
          <span title={path}>{path.split("/").pop()}</span>
          <small title={path}>{path}</small>
        </div>
        {source?.type === "file" && <span className="file-preview-chip" title={t("files.contentVersion", { version: file?.version || t("common.unknown") })}>
          {formatSize(source.file.size)}{source.file.version ? ` · ${source.file.version.slice(0, 8)}` : ""}
        </span>}
        {source?.type === "git" && <span className="file-preview-chip git" title={`${t("files.diffBase")}: ${source.diff.base}`}>
          <IconGit /> {source.diff.base}
        </span>}
        {source?.type === "tool" && <span className={`file-preview-chip${source.error ? " danger" : ""}`}>{source.name}{source.error ? ` ${t("common.failed")}` : ""}</span>}
        <button type="button" className="icon-btn file-copy-path" title={clipboard.state === "idle" ? t("files.copyFilePath") : clipboard.state === "copied" ? t("common.copied") : t("chat.copyFailed")}
          aria-label={t("files.copyFilePath")} onClick={() => void clipboard.copy(path)}><IconCopy /></button>
        {source && source.type !== "file" && onOpenFile && <button type="button" className="btn small ghost" onClick={() => onOpenFile(path)}>{t("files.openInFile")}</button>}
      </div>
      <div className="file-preview-scroll" ref={scroller} onScroll={event => { if (!loading && source) onScroll?.(event.currentTarget.scrollTop); }}>
        {error && source && <p className="file-error" role="status">{t("files.refreshFailed", { error })}</p>}
        {loading ? <div className="file-state" role="status"><span className="file-state-spinner" />{t("files.openingFile")}</div>
          : error && !source ? <p className="file-error" role="alert">{error}</p>
          : source?.type === "file" ? <FileContent key={identity} file={source.file} lines={lines} onLines={setLines} onOpen={onOpenFile ?? (() => {})} />
          : source?.type === "git" ? <>
            <div className="caption file-base-note">
              <span>{t("files.diffBase")}</span>
              <code>{source.diff.base}</code>
              {source.diff.truncated && <span className="tag warn">{t("files.truncated")}</span>}
            </div>
            <DiffView key={source.diff.path} patch={source.diff.patch} />
          </>
          : source?.type === "tool" ? <>
            <p className="file-notice">{source.error ? t("files.toolOperationNoticeFailed", { name: source.name }) : t("files.toolOperationNotice", { name: source.name })}</p>
            <DiffView patch={source.patch} emptyText={t("files.noToolDiff")} />
          </> : null}
      </div>
      {file && onReference && <div className="file-reference">
        <div className="file-reference-head">
          <span>{lines ? <><b>L{Math.min(...lines)}–{Math.max(...lines)}</b> {t("files.selected")}</> : t("files.referenceCurrentFile")}</span>
          {lines && <button type="button" className="btn small ghost" onClick={() => setLines(null)}>{t("files.clearSelection")}</button>}
        </div>
        {referenceOptions}
        <textarea className="text-input" aria-label={t("files.referenceComment")} placeholder={t("files.referenceCommentPlaceholder")} value={comment} onChange={e => setComment(e.target.value)} rows={2} />
        <div className="file-reference-actions">
          <button type="button" className="btn primary" onClick={reference} disabled={referenceDisabled}>{resolvedReferenceLabel}</button>
          <p role="status">{feedback}</p>
        </div>
      </div>}
    </> : <div className="empty-state file-preview-empty">
      <span className="icon-tile file-empty-icon"><IconFile /></span>
      <strong>{resolvedEmptyTitle}</strong>
      <p>{resolvedEmptyHint}</p>
    </div>}
  </section>;
}

export function formatSize(bytes: number): string { return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MiB` : bytes >= 1024 ? `${(bytes / 1024).toFixed(1)} KiB` : `${bytes} B`; }
