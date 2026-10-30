import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type RefObject,
} from "react";
import { formatRuntimeError } from "../chat-runtime";
import type { DraftBuffer } from "../drafts";
import { prepareAttachments } from "./attachments";
import { useI18n } from "../i18n";
import { IconArrowDown } from "../icons";
import ComposerQueue from "./ComposerQueue";
import DelayPicker from "./DelayPicker";
import SendAction from "./SendAction";
import ModelEffortPicker from "./ModelEffortPicker";
import type { ModelConfig, PromptQueueSnapshot, ReasoningEffort, UserImage } from "../types";

interface ChatComposerProps {
  containerRef: RefObject<HTMLDivElement | null>;
  draft: DraftBuffer;
  ready: boolean;
  running: boolean;
  stopping: boolean;
  uncertain?: boolean;
  temporary: boolean;
  atTail: boolean;
  model: ModelConfig | null;
  customModel: boolean;
  composingRef: RefObject<boolean>;
  onSend: (text: string, images: UserImage[]) => Promise<boolean>;
  onSteer: (text: string, images: UserImage[]) => Promise<boolean>;
  onQueue: (text: string, delayMs: number, images: UserImage[]) => Promise<boolean>;
  queue: PromptQueueSnapshot;
  onCancelQueued: (id: string) => Promise<void>;
  onPauseQueue: (paused: boolean) => Promise<void>;
  onBeforeSend: () => void;
  onStop: () => void;
  onOpenModel: () => void;
  onReasoningChange: (effort: ReasoningEffort | null) => Promise<void>;
  onScrollToLatest: () => void;
  onError: (message: string) => void;
}

/** 只拥有输入交互与提交反馈；草稿及运行状态仍由会话控制器拥有。 */
export default function ChatComposer({
  containerRef,
  draft,
  ready,
  running,
  stopping,
  uncertain = false,
  temporary,
  atTail,
  model,
  customModel,
  composingRef,
  onSend,
  onSteer,
  onQueue,
  queue,
  onCancelQueued,
  onPauseQueue,
  onBeforeSend,
  onStop,
  onOpenModel,
  onReasoningChange,
  onScrollToLatest,
  onError,
}: ChatComposerProps) {
  const { locale, t } = useI18n();
  const { text, files } = useSyncExternalStore(draft.subscribe, draft.getSnapshot);
  const [delayMinutes, setDelayMinutes] = useState("0");
  const delayOpen = delayMinutes !== "0";
  const delayMs = delayOpen ? Number(delayMinutes) * 60_000 : 0;
  const validDelay = !delayOpen || (delayMinutes.trim() !== "" && Number.isInteger(Number(delayMinutes)) && Number.isSafeInteger(delayMs) && delayMs >= 60_000 && delayMs <= 30 * 86400000);
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const [updatingReasoning, setUpdatingReasoning] = useState(false);
  const updatingReasoningRef = useRef(false);
  const mountedRef = useRef(true);
  const compositionEndedAt = useRef(0);
  const input = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  useEffect(() => { setDelayMinutes("0"); }, [draft]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      composingRef.current = false;
    };
  }, [composingRef]);
  useLayoutEffect(() => {
    if (!input.current) return;
    input.current.style.height = "auto";
    input.current.style.height = `${Math.min(input.current.scrollHeight, 180)}px`;
  }, [text]);

  const send = async (steering = false) => {
    if (!ready || uncertain || stopping || (!text.trim() && !files.length) || sendingRef.current || updatingReasoningRef.current || (!steering && !validDelay)) return;
    sendingRef.current = true;
    setSending(true);
    onBeforeSend();
    try {
      const snapshot = draft.getSnapshot();
      const prepared = await prepareAttachments(snapshot.text, snapshot.files);
      const accepted = await (steering ? onSteer(prepared.text, prepared.images)
        : delayOpen || running || queue.messages.length ? onQueue(prepared.text, delayMs, prepared.images) : onSend(prepared.text, prepared.images));
      if (accepted) draft.acknowledge(snapshot.revision);
      if (accepted && !steering && mountedRef.current) setDelayMinutes("0");
    } catch (error) {
      if (mountedRef.current) onError(formatRuntimeError(error));
    } finally {
      sendingRef.current = false;
      if (mountedRef.current) setSending(false);
    }
  };

  const changeReasoning = async (effort: ReasoningEffort | null) => {
    if (!ready || running || stopping || uncertain || !model || sendingRef.current || updatingReasoningRef.current) return false;
    updatingReasoningRef.current = true;
    setUpdatingReasoning(true);
    try {
      await onReasoningChange(effort);
      return true;
    } catch (error) {
      if (mountedRef.current) onError(formatRuntimeError(error));
      return false;
    } finally {
      updatingReasoningRef.current = false;
      if (mountedRef.current) setUpdatingReasoning(false);
    }
  };

  const sendLabel = uncertain ? t("chat.sendPending")
    : delayOpen ? (locale === "en" ? "Send later" : "延迟发送")
    : running || queue.messages.length ? (locale === "en" ? "Queue" : "排队") : t("chat.send");
  const modelName = model?.id || (locale === "en" ? "No model" : "未配置模型");
  return (
    <div className="composer" ref={containerRef}>
      {!atTail && (
        <button
          type="button"
          className="jump-to-latest"
          onClick={onScrollToLatest}
        >
          <IconArrowDown />
          {locale === "en" ? "Latest messages" : "回到最新消息"}
        </button>
      )}
      {temporary && (
        <div className="caption test-safety-note" role="note">
          <span>临时仅指对话记录</span>
          工具仍按当前权限真实运行，对工作区造成的改动会保留。
        </div>
      )}
      <ComposerQueue queue={queue} onCancel={onCancelQueued} onPause={onPauseQueue} onError={onError} />
      <div className={`composer-box${running ? " is-running" : ""}`}>
        {!!files.length && <div className="composer-attachments">{files.map((file, index) => <span className="composer-attachment" key={`${file.name}-${index}`} title={file.name}><span className="truncate">{file.name}</span><button type="button" aria-label={`移除 ${file.name}`} onClick={() => draft.setFiles(files.filter((_, i) => i !== index))}>×</button></span>)}</div>}
        <textarea
          ref={input}
          value={text}
          aria-label={temporary ? "临时测试消息" : "会话消息"}
          onChange={(event) => draft.write(event.target.value)}
          disabled={!ready}
          placeholder={
            ready
              ? running
                ? locale === "en"
                  ? "Guide the current task, or queue the next message…"
                  : "引导当前任务，或排队发送下一条消息…"
                : locale === "en"
                  ? "Describe what you want to get done…"
                  : "描述你想完成的工作…"
              : locale === "en"
                ? "Connecting to Agent…"
                : "正在连接 Agent…"
          }
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
            compositionEndedAt.current = Date.now();
          }}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.shiftKey) return;
            const native = event.nativeEvent;
            if (
              composingRef.current ||
              native.isComposing ||
              native.keyCode === 229 ||
              Date.now() - compositionEndedAt.current < 100
            )
              return;
            event.preventDefault();
            void send(event.altKey && running);
          }}
        />
        <div className="composer-bar">
          <input ref={fileInput} type="file" multiple hidden onChange={event => { const picked = Array.from(event.target.files ?? []); if (picked.length) draft.setFiles([...draft.getSnapshot().files, ...picked]); event.target.value = ""; }} />
          <button type="button" className="composer-attach" disabled={!ready || sending} title={locale === "en" ? "Attach images or text files" : "添加图片或文本文件"} aria-label={locale === "en" ? "Attach files" : "添加文件"} onClick={() => fileInput.current?.click()}>+</button>
          {!temporary ? <ModelEffortPicker
            model={model}
            customModel={customModel}
            disabled={!ready || running || stopping || sending || updatingReasoning || uncertain}
            disabledReason={running ? (locale === "en" ? "Cannot change model while running" : "Agent 运行中不可切换模型") : undefined}
            onOpenModel={onOpenModel}
            onEffortChange={changeReasoning}
          /> : <span className="truncate caption test-model" title={modelName}>{modelName}</span>}
          <span className="spacer" />
          <span className="caption composer-state" role="status">
            {updatingReasoning
              ? locale === "en" ? "Saving…" : "正在保存…"
              : stopping
              ? locale === "en"
                ? "Stopping…"
                : "正在停止…"
              : sending
                ? locale === "en"
                  ? "Sending…"
                  : "正在提交…"
                : uncertain
                  ? t("chat.sendPending")
                  : null}
          </span>
          <DelayPicker value={delayMinutes} en={locale === "en"} onChange={setDelayMinutes} />
          <SendAction
            en={locale === "en"}
            running={running}
            stopping={stopping}
            busy={uncertain || sending || stopping}
            stopLabel={stopping ? (locale === "en" ? "Stopping…" : "正在停止…") : t("chat.stop")}
            sendLabel={sendLabel}
            steerDisabled={!running || !ready || uncertain || (!text.trim() && !files.length) || sending || stopping || updatingReasoning}
            sendDisabled={!ready || uncertain || (!text.trim() && !files.length) || sending || stopping || updatingReasoning || !validDelay}
            onStop={onStop}
            onSteer={() => void send(true)}
            onSend={() => void send()}
          />
        </div>
      </div>
    </div>
  );
}
