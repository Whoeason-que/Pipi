import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type RefObject,
} from "react";
import {
  formatRuntimeError,
  type ApprovalDecisionValue,
  type ApprovalRequestPayload,
} from "../chat-runtime";
import { submitDraft, type DraftBuffer } from "../drafts";
import { useI18n } from "../i18n";
import { IconArrowDown, IconSend, IconStop } from "../icons";
import type { ModelConfig } from "../types";

interface ChatComposerProps {
  containerRef: RefObject<HTMLDivElement | null>;
  draft: DraftBuffer;
  ready: boolean;
  running: boolean;
  stopping: boolean;
  temporary: boolean;
  atTail: boolean;
  model: ModelConfig | null;
  customModel: boolean;
  approval: ApprovalRequestPayload | null;
  composingRef: RefObject<boolean>;
  onSend: (text: string) => Promise<boolean>;
  onBeforeSend: () => void;
  onStop: () => void;
  onResolveApproval: (decision: ApprovalDecisionValue) => void;
  onOpenModel: () => void;
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
  temporary,
  atTail,
  model,
  customModel,
  approval,
  composingRef,
  onSend,
  onBeforeSend,
  onStop,
  onResolveApproval,
  onOpenModel,
  onScrollToLatest,
  onError,
}: ChatComposerProps) {
  const { locale, t } = useI18n();
  const { text } = useSyncExternalStore(draft.subscribe, draft.getSnapshot);
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const mountedRef = useRef(true);
  const compositionEndedAt = useRef(0);
  const input = useRef<HTMLTextAreaElement>(null);

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

  const send = async () => {
    if (!ready || !text.trim() || sendingRef.current) return;
    sendingRef.current = true;
    setSending(true);
    onBeforeSend();
    try {
      await submitDraft(draft, onSend);
    } catch (error) {
      if (mountedRef.current) onError(formatRuntimeError(error));
    } finally {
      sendingRef.current = false;
      if (mountedRef.current) setSending(false);
    }
  };

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
      {approval && (
        <div
          className="approval-bar"
          role="alertdialog"
          aria-label="命令执行审批"
        >
          <div className="approval-text">
            <span className="approval-title">Agent 请求执行白名单外的命令</span>
            <code>{approval.command}</code>
          </div>
          <div className="approval-actions">
            <button
              type="button"
              className="btn deny"
              onClick={() => onResolveApproval("deny")}
            >
              拒绝
            </button>
            <button
              type="button"
              className="btn allow"
              onClick={() => onResolveApproval("allow")}
            >
              允许一次
            </button>
            <button
              type="button"
              className="btn allow"
              onClick={() => onResolveApproval("always")}
              title="执行并把命令加入 Agent 白名单（agent.json）"
            >
              总是允许
            </button>
          </div>
        </div>
      )}
      {temporary && (
        <div className="test-safety-note" role="note">
          <span>临时仅指对话记录</span>
          工具仍按当前权限真实运行，对工作区造成的改动会保留。
        </div>
      )}
      <div className={`composer-box${running ? " is-running" : ""}`}>
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
                  ? "Adjust the direction while your Agent works…"
                  : "Agent 正在工作，可以在这里插话调整方向…"
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
            void send();
          }}
        />
        <div className="composer-bar">
          {!temporary ? (
            <button
              type="button"
              className="model-pill composer-model-pill"
              onClick={onOpenModel}
              disabled={running}
              title={
                running
                  ? locale === "en"
                    ? "Cannot change model while running"
                    : "Agent 运行中不可切换模型"
                  : locale === "en"
                    ? "Switch session model"
                    : "切换当前会话模型"
              }
            >
              <span className="model-indicator" />
              <span className="model-pill-name">{modelName}</span>
              {customModel && (
                <span className="model-pill-tag">
                  {locale === "en" ? "Custom" : "自定义"}
                </span>
              )}
              <span className="model-chevron" aria-hidden="true">
                ⌄
              </span>
            </button>
          ) : (
            <span className="test-model composer-model-pill" title={modelName}>
              {modelName}
            </span>
          )}
          <span className="spacer" />
          <span className="composer-state" role="status">
            {stopping
              ? locale === "en"
                ? "Stopping…"
                : "正在停止…"
              : sending
                ? locale === "en"
                  ? "Sending…"
                  : "正在提交…"
                : running
                  ? locale === "en"
                    ? "Running · Steering available"
                    : "运行中 · 可插话"
                  : locale === "en"
                    ? "Enter to send"
                    : "Enter 发送"}
          </span>
          {running && (
            <button
              type="button"
              className="btn stop"
              onClick={onStop}
              disabled={stopping}
              title={
                stopping
                  ? locale === "en"
                    ? "Stopping…"
                    : "正在停止…"
                  : t("chat.stop")
              }
              aria-label={t("chat.stop")}
            >
              <IconStop />
            </button>
          )}
          <button
            type="button"
            className="btn send"
            disabled={!ready || !text.trim() || sending || stopping}
            onClick={() => void send()}
            title={
              running
                ? locale === "en"
                  ? "Send steering"
                  : "插话"
                : t("chat.send")
            }
            aria-label={
              running ? (locale === "en" ? "Steering" : "插话") : t("chat.send")
            }
          >
            <IconSend />
          </button>
        </div>
      </div>
    </div>
  );
}
