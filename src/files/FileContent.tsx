import { memo, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import { codeFence, languageFor, linkedPath } from "./model";
import type { WorkspaceFile } from "./types";

function useMediaUrl(file: WorkspaceFile): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!file.base64) { setUrl(null); return; }
    const bytes = Uint8Array.from(atob(file.base64), char => char.charCodeAt(0));
    const next = URL.createObjectURL(new Blob([bytes], { type: file.mime }));
    setUrl(next);
    return () => { URL.revokeObjectURL(next); };
  }, [file]);
  return url;
}

interface Props {
  file: WorkspaceFile;
  lines: [number, number] | null;
  onLines: (lines: [number, number] | null) => void;
  onOpen: (path: string) => void;
}

export const FileContent = memo(function FileContent({ file, lines, onLines, onOpen }: Props) {
  const url = useMediaUrl(file);
  const [rendered, setRendered] = useState(file.kind === "markdown");
  const [query, setQuery] = useState("");
  const [matchIndex, setMatchIndex] = useState(0);
  const source = useRef<HTMLDivElement>(null);
  const content = file.content;
  const textLines = content?.split("\n") ?? [];
  const matches = query ? textLines.flatMap((text, index) => text.toLowerCase().includes(query.toLowerCase()) ? [index + 1] : []) : [];
  const match = matches[matchIndex % (matches.length || 1)];
  useEffect(() => {
    if (match) source.current?.querySelector(`[data-line="${match}"]`)?.scrollIntoView({ block: "center" });
  }, [match]);

  if (url) return <div className="file-media">
    {file.kind === "image" && <img src={url} alt={file.path} />}
    {file.kind === "audio" && <audio controls src={url} preload="metadata" />}
    {file.kind === "video" && <video controls src={url} preload="metadata" />}
    {file.kind === "pdf" && <object data={url} type="application/pdf" aria-label={file.path}>
      <p>当前环境不支持内嵌 PDF，请下载后打开。</p>
    </object>}
    <a href={url} download={file.path.split("/").pop()}>下载文件</a>
  </div>;
  if (content === null) return <p className="file-notice">{file.truncated && file.kind !== "binary" ? "文件超过 8 MiB 媒体预览上限。" : file.base64 ? "正在准备预览…" : "此二进制格式暂不支持预览。Office 文档支持将在后续补充。"}</p>;
  // 极端的短行文件也要限制 DOM 数量；用户仍可引用文件路径。
  if (textLines.length > 10_000) return <p className="file-notice">文本超过 10000 行，请使用外部编辑器查看。</p>;

  return <>
    <div className="file-content-toolbar">
      {file.kind === "markdown" && <button type="button" className="btn small" onClick={() => setRendered(!rendered)}>{rendered ? "查看源码" : "渲染预览"}</button>}
      {!rendered && <>
        <input className="text-input" aria-label="查找文件内容" placeholder="查找内容…" value={query} onChange={e => { setQuery(e.target.value); setMatchIndex(0); }}
          onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); setMatchIndex(i => i + (e.shiftKey ? Math.max(1, matches.length - 1) : 1)); } }} />
        <button type="button" className="btn small ghost" disabled={!matches.length} onClick={() => setMatchIndex(i => i + 1)}>{matches.length ? `${matchIndex % matches.length + 1}/${matches.length}` : "0"} ↓</button>
      </>}
    </div>
    {file.truncated && <p className="file-notice">仅展示前 256 KiB；截断内容不能作为完整文件版本或选区引用。</p>}
    {rendered ? <div className="md file-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeHighlight]}
      components={{
        img: ({ alt, src }) => { const path = linkedPath(file.path, src ?? ""); return path ? <button type="button" className="btn small" onClick={() => onOpen(path)}>打开图片：{alt || path}</button> : <span>[外部图片：{alt || "未自动加载"}]</span>; },
        a: ({ href, children }) => { const path = linkedPath(file.path, href ?? ""); return path ? <button type="button" className="file-link" onClick={() => onOpen(path)}>{children}</button> : /^https?:\/\//i.test(href ?? "") ? <a href={href} target="_blank" rel="noreferrer noopener">{children}</a> : <span>{children}</span>; },
      }}>{content}</ReactMarkdown></div> : <>
      <p className="file-selection-hint">点击行号选择一行，Shift + 点击选择范围。</p>
      <div className="file-source" ref={source}>
        <div className="file-line-numbers" aria-label="选择代码行">
          {textLines.map((_, index) => {
            const line = index + 1;
            const selected = lines && line >= Math.min(...lines) && line <= Math.max(...lines);
            return <button type="button" key={line} data-line={line} className={`${selected ? "selected" : ""}${line === match ? " matched" : ""}`}
              aria-label={`第 ${line} 行`} aria-pressed={!!selected} disabled={file.truncated}
              onClick={e => onLines(e.shiftKey && lines ? [lines[0], line] : [line, line])}>{line}</button>;
          })}
        </div>
        <div className="file-source-code md"><ReactMarkdown rehypePlugins={[[rehypeHighlight, { detect: false }]]}>{codeFence(content, languageFor(file.path))}</ReactMarkdown></div>
      </div>
    </>}
  </>;
});
