import { useMemo, useState } from "react";
import type { ChatEntry } from "../chat-runtime";
import { useI18n } from "../i18n";
import { FileGlyph } from "./FileGlyph";
import { FilePreview, type PreviewSource } from "./FilePreview";
import { toolFiles } from "./model";

/** 会话只拥有操作记录；磁盘当前内容交由独立 File 页面读取。 */
export function SessionFilesPanel({ entries, onOpenFile, active }: { entries: ChatEntry[]; onOpenFile?: (path: string) => void; active: boolean }) {
  const { t } = useI18n();
  // 面板不可见时不扫描 entries；组件保持挂载，operationId 选择状态不丢失。
  const files = useMemo(() => (active ? toolFiles(entries) : []), [entries, active]);
  const [operationId, setOperationId] = useState<string | null>(null);
  if (!active) return null;
  let source: PreviewSource | null = null;
  for (const file of files) {
    const operation = file.operations.find(item => item.id === operationId);
    if (operation?.patch) source = { type: "tool", path: file.path, ...operation, patch: operation.patch };
  }
  return <div className="file-workbench session-files-panel">
    <div className="file-workbench-header"><strong>{t("inspector.files")}</strong><small>{t("files.fileCount", { count: files.length })}</small></div>
    <p className="file-notice">{t("files.sessionFilesHint")}</p>
    {!files.length && <p className="file-notice">{t("files.noFileOperations")}</p>}
    {files.map(file => <div className="file-tool" key={file.path}>
      {onOpenFile ? <button type="button" className="file-row" title={file.path} onClick={() => onOpenFile(file.path)}><FileGlyph path={file.path} size="sm" /><span>{file.path}</span><small>{t("files.openInFile")}</small></button> : <div className="file-row"><FileGlyph path={file.path} size="sm" /><span>{file.path}</span></div>}
      <p className="file-notice">{t("files.operationCount", { count: file.calls })}{file.errors ? ` · ${t("files.errorCount", { count: file.errors })}` : ""}</p>
      {file.operations.filter(operation => operation.patch).map((operation, index) => <button type="button" className="btn small file-operation" key={operation.id} aria-pressed={operation.id === operationId}
        onClick={() => setOperationId(operation.id === operationId ? null : operation.id)}>{operation.error ? t("files.operationDiffFailed", { name: operation.name, index: index + 1 }) : t("files.operationDiff", { name: operation.name, index: index + 1 })}</button>)}
    </div>)}
    {source && <FilePreview path={source.type === "tool" ? source.path : null} source={source} onOpenFile={onOpenFile} />}
  </div>;
}
