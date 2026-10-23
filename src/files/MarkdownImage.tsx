import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useDialogFocus } from "../components/use-dialog-focus";
import { formatRuntimeError } from "../chat-runtime";
import { invoke } from "../platform";
import { useI18n } from "../i18n";
import type { AgentDefinition } from "../types";
import { markdownImagePath } from "./model";

export type ImageAgent = Pick<AgentDefinition, "name" | "workspace">;
interface ImageState { identity: string; url: string; error: string }

/** 文件内容只经现有鉴权 IPC 读取；blob 随消息卸载释放，不把磁盘路径交给浏览器。 */
export function MarkdownImage({ src = "", alt = "", agent }: { src?: string; alt?: string; agent?: ImageAgent }) {
  const { t } = useI18n();
  const path = markdownImagePath(src);
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<ImageState | null>(null);
  const [open, setOpen] = useState(false);
  const dialog = useRef<HTMLDivElement>(null);
  const identity = JSON.stringify([agent?.name, agent?.workspace, path, revision]);
  const current = state?.identity === identity ? state : null;
  const url = current?.url ?? "";
  const error = !path ? t("chat.imageUnsupported") : !agent ? t("chat.imageNoAgent") : current?.error ?? "";
  const name = path?.split(/[\\/]/).pop() || alt || t("chat.image");
  useDialogFocus(dialog, () => setOpen(false), open && !!url && !error);

  useEffect(() => {
    setOpen(false);
    if (!agent || !path) return;
    let disposed = false;
    let objectUrl = "";
    void invoke("preview_workspace_file", { agentName: agent.name, path }).then(file => {
      if (disposed) return;
      if (file.truncated) throw new Error(t("chat.imageTooLarge"));
      if (file.kind !== "image" || !["image/png", "image/jpeg", "image/gif", "image/webp"].includes(file.mime) || !file.base64) {
        throw new Error(t("chat.imageUnsupported"));
      }
      const bytes = Uint8Array.from(atob(file.base64), char => char.charCodeAt(0));
      objectUrl = URL.createObjectURL(new Blob([bytes], { type: file.mime }));
      setState({ identity, url: objectUrl, error: "" });
    }).catch(error => {
      if (!disposed) setState({ identity, url: "", error: formatRuntimeError(error) });
    });
    return () => { disposed = true; if (objectUrl) URL.revokeObjectURL(objectUrl); };
    // identity 包含 Agent、工作区、路径与显式重试次数，流式文本变化不重复读取。
  }, [identity]);

  const imageError = () => setState(previous => previous?.identity === identity
    ? { ...previous, error: t("chat.imageDecodeError") } : previous);

  return <span className="chat-image">
    {url && !error ? <>
      <button type="button" className="chat-image-button" onClick={() => setOpen(true)} aria-label={`${t("chat.imageEnlarge")}：${alt || name}`}>
        <img src={url} alt={alt || name} loading="lazy" onError={imageError} />
      </button>
      <span className="chat-image-caption">
        <span title={path ?? undefined}>{name}</span>
        <a href={url} download={name}>{t("chat.imageDownload")}</a>
      </span>
    </> : <span className="chat-image-notice" role="status">
      <span>{error ? `${alt || name}：${error}` : t("chat.imageLoading")}</span>
      {error && path && agent && <button type="button" className="btn small ghost" onClick={() => setRevision(value => value + 1)}>{t("common.retry")}</button>}
    </span>}
    {open && url && !error && createPortal(
      <div className="chat-image-backdrop" onClick={event => { if (event.target === event.currentTarget) setOpen(false); }}>
        <div className="chat-image-dialog" role="dialog" aria-modal="true" aria-label={alt || name} ref={dialog} tabIndex={-1}>
          <div className="chat-image-caption">
            <span title={path ?? undefined}>{name}</span>
            <a href={url} download={name} tabIndex={0}>{t("chat.imageDownload")}</a>
            <button type="button" className="btn small" onClick={() => setOpen(false)}>{t("common.close")}</button>
          </div>
          <img src={url} alt={alt || name} onError={imageError} />
        </div>
      </div>, document.body,
    )}
  </span>;
}
