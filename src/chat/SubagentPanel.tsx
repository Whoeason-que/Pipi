import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { SessionRuntime } from "../session-runtime";
import { formatRuntimeError, type ChatEntry } from "../chat-runtime";
import { useI18n } from "../i18n";
import { invoke } from "../platform";
import { IconSubagent } from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { formatClock } from "./ToolDetails";

interface Props {
  agent: AgentDefinition;
  /** 当前查看的父会话；null = 新会话，还没有子运行。 */
  sessionId: string | null;
  entries: ChatEntry[];
  parentRunning: boolean;
  runtime: SessionRuntime;
  /** 只有切到这个 tab 时才拉取与轮询。 */
  active: boolean;
}

/** 右侧栏的 Subagent 预览：本会话 run_agent 派生的 child session 列表 + 最近输出。 */
export default function SubagentPanel({ agent, sessionId, entries, parentRunning, runtime, active }: Props) {
  const { t } = useI18n();
  const [runs, setRuns] = useState<SessionSummaryView[]>([]);
  const [runningIds, setRunningIds] = useState<Set<string>>(() => new Set());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef(0);
  const messageCount = (count: number) =>
    t(count === 1 ? "inspector.subagentMessageCountOne" : "inspector.subagentMessageCount", { count });

  const refresh = useCallback(async () => {
    if (!sessionId) {
      request.current += 1;
      setRuns([]);
      setLoading(false);
      setError(null);
      return;
    }
    const id = ++request.current;
    setLoading(true);
    try {
      const [sessionsResult, infosResult] = await Promise.allSettled([
        invoke("list_sessions", { agentName: agent.name }),
        invoke("session_infos"),
      ]);
      if (id !== request.current) return;
      if (sessionsResult.status === "rejected") throw sessionsResult.reason;
      const infos = infosResult.status === "fulfilled" ? infosResult.value : [];
      setRuns(sessionsResult.value
        .filter((session) => session.parentSessionId === sessionId)
        .sort((left, right) => right.lastActive - left.lastActive));
      setRunningIds(new Set(infos
        .filter((info) => info.agentName === agent.name && info.running)
        .map((info) => info.sessionId)));
      setError(null);
    } catch (errorValue) {
      if (id === request.current) setError(formatRuntimeError(errorValue));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, [agent.name, sessionId]);

  // 切换父会话时先清掉上一个会话的子运行，避免加载期间串台。
  useEffect(() => {
    setRuns([]);
    setSelectedId(null);
    setError(null);
  }, [sessionId]);

  // 父会话每产生一条新条目（尤其是工具调用/结果）都可能出现新的 child session。
  useEffect(() => { if (active) void refresh(); }, [active, refresh, entries.length]);

  // 子运行的创建（session-changed）与起止（agent_start/agent_end）都走全局事件。
  useEffect(() => {
    if (!active) return;
    return runtime.subscribe((frame) => {
      if (frame.type === "session-changed") {
        void refresh();
      } else if (frame.type === "agent-event"
        && (frame.payload.event.type === "agent_start" || frame.payload.event.type === "agent_end")) {
        void refresh();
      }
    });
  }, [active, runtime, refresh]);

  // 运行中低频兜底轮询；子会话正文由 controller 的实时事件更新。
  useEffect(() => {
    if (!active || !parentRunning) return;
    const timer = setInterval(() => void refresh(), 5000);
    return () => clearInterval(timer);
  }, [active, parentRunning, refresh]);

  useEffect(() => {
    setSelectedId((current) => current && runs.some((run) => run.id === current) ? current : runs[0]?.id ?? null);
  }, [runs]);

  const selected = runs.find((run) => run.id === selectedId) ?? null;

  return <div className="subagent-panel">
    <section className="ip">
      <h4>
        {t("inspector.subagentRuns")}
        {runs.length > 0 && <span className="subagent-count">{runs.length}</span>}
      </h4>
      {error && <p className="file-error" role="status">{t("inspector.subagentLoadFailed", { error })}</p>}
      {runs.length === 0 ? (
        loading
          ? <div className="subagent-loading" role="status"><span className="file-state-spinner" />{t("common.loading")}</div>
          : <div className="subagent-empty">
            <span className="icon-tile subagent-empty-icon"><IconSubagent /></span>
            <strong>{t("inspector.subagentEmptyTitle")}</strong>
            <p>{t("inspector.subagentEmptyHint")}</p>
          </div>
      ) : (
        <div className="subagent-run-list">
          {runs.map((run) => {
            const running = runningIds.has(run.id) || runtime.isRunning(agent.name, run.id);
            return <button
              key={run.id}
              type="button"
              className={`subagent-run${run.id === selectedId ? " active" : ""}`}
              aria-label={`${run.title || t("chat.newSessionTitle")} · ${running ? t("common.running") : t("chat.statusDone")}`}
              onClick={() => setSelectedId(run.id)}
            >
              <span className={`subagent-dot${running ? " running" : ""}`} aria-hidden="true" />
              <span className="subagent-run-main">
                <span className="truncate subagent-run-title">{run.title || t("chat.newSessionTitle")}</span>
                <span className="subagent-run-meta">
                  <span>{messageCount(run.messageCount)}</span>
                  {run.lastActive > 0 && <span>{formatClock(run.lastActive)}</span>}
                </span>
              </span>
            </button>;
          })}
        </div>
      )}
    </section>
    {selected && <SubagentPreview
      key={selected.id}
      run={selected}
      agent={agent}
      runtime={runtime}
      running={runningIds.has(selected.id) || runtime.isRunning(agent.name, selected.id)}
    />}
  </div>;
}

function SubagentPreview({ run, agent, runtime, running }: {
  run: SessionSummaryView;
  agent: AgentDefinition;
  runtime: SessionRuntime;
  running: boolean;
}) {
  const { t } = useI18n();
  const controller = useMemo(
    () => runtime.get(agent, run.id),
    [runtime, agent, run.id],
  );
  useEffect(() => { void controller.reconnect(); }, [controller]);
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const entries = snapshot.chat.entries.filter(isPreviewEntry).slice(-10);
  const liveRunning = running || snapshot.chat.running;
  const count = Math.max(run.messageCount, snapshot.chat.entries.length);
  const loadingSession = snapshot.load === "loading" || snapshot.load === "idle";
  const messageCount = (value: number) =>
    t(value === 1 ? "inspector.subagentMessageCountOne" : "inspector.subagentMessageCount", { count: value });
  const body = useRef<HTMLDivElement>(null);
  const lastKey = entries.at(-1)?.key;
  const lastText = entries.at(-1)?.text;

  // 预览跟随最新输出；用户切回来时也停在底部。
  useEffect(() => {
    const element = body.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [lastKey, lastText, liveRunning]);

  return <section className="ip">
    <h4>{t("inspector.subagentPreview")}</h4>
    <div className="subagent-preview-head">
      <span className={`subagent-dot${liveRunning ? " running" : ""}`} aria-hidden="true" />
      <strong className="truncate" title={run.title}>{run.title || t("chat.newSessionTitle")}</strong>
      <span className={`subagent-status${liveRunning ? " running" : ""}`}>
        {liveRunning ? t("common.running") : t("chat.statusDone")}
      </span>
    </div>
    <div className="subagent-preview-meta">
      <span>{messageCount(count)}</span>
      {run.lastActive > 0 && <span>{formatClock(run.lastActive)}</span>}
    </div>
    {loadingSession && entries.length === 0
      ? <div className="subagent-loading" role="status"><span className="file-state-spinner" />{t("common.loading")}</div>
      : snapshot.load === "failed"
        ? <p className="file-error" role="alert">{t("inspector.subagentLoadFailed", { error: snapshot.error?.message ?? "" })}</p>
        : entries.length === 0
          ? <p className="ip-note">{liveRunning ? t("common.loading") : t("inspector.subagentNoOutput")}</p>
          : <div className="subagent-preview-body" ref={body}>
            {entries.map((entry) => <SubagentEntry key={entry.key} entry={entry} />)}
          </div>}
  </section>;
}

function isPreviewEntry(entry: ChatEntry): boolean {
  return (entry.role === "user" || entry.role === "assistant" || entry.role === "toolResult")
    && (entry.text.trim().length > 0 || Boolean(entry.toolName));
}

function SubagentEntry({ entry }: { entry: ChatEntry }) {
  const { t } = useI18n();
  const role = entry.role === "user"
    ? t("inspector.userRole")
    : entry.role === "assistant"
      ? t("inspector.assistantRole")
      : entry.toolName ?? "tool";
  const status = entry.toolRunning ? t("common.running") : entry.isError ? t("common.failed") : "";
  return <div className={`subagent-entry ${entry.role === "toolResult" ? "tool" : entry.role}`}>
    <span className="subagent-entry-role">
      {role}
      {status && <span className={`subagent-entry-status${entry.isError ? " failed" : entry.toolRunning ? " running" : ""}`}> · {status}</span>}
    </span>
    {entry.text.trim() && <span className="subagent-entry-text">{entry.text}</span>}
  </div>;
}
