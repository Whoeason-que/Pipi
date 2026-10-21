import { useMemo, useState } from "react";
import type { ChatEntry } from "../chat-runtime";
import { FilePreview, type PreviewSource } from "./FilePreview";
import { toolFiles } from "./model";

/** 会话只拥有操作记录；磁盘当前内容交由独立 File 页面读取。 */
export function SessionFilesPanel({ entries, onOpenFile }: { entries: ChatEntry[]; onOpenFile?: (path: string) => void }) {
  const files = useMemo(() => toolFiles(entries), [entries]);
  const [operationId, setOperationId] = useState<string | null>(null);
  let source: PreviewSource | null = null;
  for (const file of files) {
    const operation = file.operations.find(item => item.id === operationId);
    if (operation?.patch) source = { type: "tool", path: file.path, ...operation, patch: operation.patch };
  }
  return <div className="file-workbench session-files-panel">
    <div className="file-workbench-header"><strong>会话文件</strong><small>{files.length} 个文件</small></div>
    <p className="file-notice">仅显示本次会话 read / write / edit 涉及的文件与操作记录。</p>
    {!files.length && <p className="file-notice">本次会话还没有文件操作。</p>}
    {files.map(file => <div className="file-tool" key={file.path}>
      {onOpenFile ? <button type="button" className="file-row" title={file.path} onClick={() => onOpenFile(file.path)}><span>{file.path}</span><small>在 File 打开</small></button> : <div className="file-row"><span>{file.path}</span></div>}
      <p className="file-notice">{file.calls} 次操作{file.errors ? ` · ${file.errors} 次失败` : ""}</p>
      {file.operations.filter(operation => operation.patch).map((operation, index) => <button type="button" className="file-operation" key={operation.id} aria-pressed={operation.id === operationId}
        onClick={() => setOperationId(operation.id === operationId ? null : operation.id)}>{operation.name} 差异 {index + 1}{operation.error ? "（失败）" : ""}</button>)}
    </div>)}
    {source && <FilePreview path={source.type === "tool" ? source.path : null} source={source} onOpenFile={onOpenFile} />}
  </div>;
}
