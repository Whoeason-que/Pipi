import { useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import { Markdown } from "./Markdown";
import { invoke, listen } from "./platform";
import {
  assistantFooter,
  chatReducer,
  createEntryKey,
  eventMatchesRun,
  formatRuntimeError,
  INITIAL_CHAT_STATE,
  normalizeAgentEvent,
  normalizeStatsPayload,
  type AgentEvent,
  type AgentEventPayload,
  type MessageView,
  type SessionErrorPayload,
  type SessionEventMeta,
  type SessionStatsPayload,
} from "./chat-runtime";
import type { AgentDefinition, SessionInfoView, SessionStatsView } from "./types";

export type { MessageView } from "./chat-runtime";

interface ChatViewProps {
  agent: AgentDefinition;
  blockedSessionIds: string[];
  onBack: () => void;
  onError: (msg: string) => void;
  onNewSession: (previousSessionId?: string) => Promise<boolean>;
  onRunningChange: (running: boolean) => void;
  onSessionReset: () => void;
}

export default function ChatView({ agent, blockedSessionIds, onBack, onError, onNewSession, onRunningChange, onSessionReset }: ChatViewProps) {
  const [state, dispatch] = useReducer(chatReducer, INITIAL_CHAT_STATE);
  const { entries, stats, running } = state;
  const runningRef = useRef(running);
  runningRef.current = running;
  const [ready, setReady] = useState(false);
  const [input, setInput] = useState("");
  const [stopping, setStopping] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const mountedRef = useRef(true);
  const sessionIdentityRef = useRef<{ sessionId: string; runId?: number } | null>(null);
  const blockedSessionIdsRef = useRef(new Set(blockedSessionIds));
  const awaitingSessionIdentityRef = useRef(true);
  const sessionInfoResolvedRef = useRef(false);
  const sessionInfoFailedRef = useRef(false);
  const runEpochRef = useRef(0);
  const awaitingRunIdRef = useRef<number | null>(null);
  const settledRunIdRef = useRef<number | null>(null);
  const ignoreEventsUntilNextRunRef = useRef(false);
  const hydrationCompleteRef = useRef(false);
  const pendingIdentityEventsRef = useRef<AgentEventPayload[]>([]);
  const pendingIdentityStatsRef = useRef<SessionStatsPayload[]>([]);
  const pendingEventsRef = useRef<Array<{ type: "event"; event: AgentEvent; key: string }>>([]);
  const pendingStatsRef = useRef<SessionStatsView[]>([]);
  const stopPollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const followTailRef = useRef(true);
  const sendInFlightRef = useRef(false);
  const composingRef = useRef(false);
  const compositionEndedAtRef = useRef(0);

  const acceptMeta = (meta: SessionEventMeta | null): boolean => {
    if (meta && blockedSessionIdsRef.current.has(meta.sessionId)) return false;
    if (!meta && (
      awaitingSessionIdentityRef.current
      || settledRunIdRef.current !== null
      || awaitingRunIdRef.current !== null
    )) return false;
    if (!eventMatchesRun(
      meta,
      agent.name,
      sessionIdentityRef.current,
      settledRunIdRef.current,
      ignoreEventsUntilNextRunRef.current,
    )) return false;
    if (meta && awaitingRunIdRef.current !== null) {
      if (meta.runId <= awaitingRunIdRef.current) return false;
      awaitingRunIdRef.current = null;
      settledRunIdRef.current = null;
    }
    if (!meta) return true;

    const identity = sessionIdentityRef.current;
    if (identity && meta.runId < (identity.runId ?? 0)) return false;
    if (identity && meta.runId > (identity.runId ?? 0)) {
      settledRunIdRef.current = null;
    }
    if (settledRunIdRef.current === meta.runId) return false;

    sessionIdentityRef.current = identity
      ? { ...identity, runId: Math.max(identity.runId ?? 0, meta.runId) }
      : { sessionId: meta.sessionId, runId: meta.runId };
    awaitingSessionIdentityRef.current = false;
    return true;
  };

  const processAgentPayload = (payload: AgentEventPayload) => {
    const normalized = normalizeAgentEvent(payload);
    if (!acceptMeta(normalized.meta)) return;
    if (normalized.event.type === "agent_end") {
      settledRunIdRef.current = normalized.meta?.runId ?? sessionIdentityRef.current?.runId ?? null;
    }
    const action = { type: "event" as const, event: normalized.event, key: createEntryKey("event") };
    if (hydrationCompleteRef.current) dispatch(action);
    else pendingEventsRef.current.push(action);
  };
  const processStatsPayload = (payload: SessionStatsPayload) => {
    const normalized = normalizeStatsPayload(payload);
    if (!acceptMeta(normalized.meta)) return;
    if (hydrationCompleteRef.current) dispatch({ type: "stats", stats: normalized.stats });
    else pendingStatsRef.current.push(normalized.stats);
  };

  useEffect(() => {
    let mounted = true;
    mountedRef.current = true;
    hydrationCompleteRef.current = false;
    sessionInfoResolvedRef.current = false;
    sessionInfoFailedRef.current = false;
    pendingIdentityEventsRef.current = [];
    pendingIdentityStatsRef.current = [];
    pendingEventsRef.current = [];
    pendingStatsRef.current = [];
    const unlisteners: Array<() => void> = [];

    const handleAgentEvent = (event: { payload: AgentEventPayload }) => {
      if (!mounted || sessionInfoFailedRef.current) return;
      if (!sessionInfoResolvedRef.current) {
        pendingIdentityEventsRef.current.push(event.payload);
        return;
      }
      processAgentPayload(event.payload);
    };
    const handleStatsEvent = (event: { payload: SessionStatsPayload }) => {
      if (!mounted || sessionInfoFailedRef.current) return;
      if (!sessionInfoResolvedRef.current) {
        pendingIdentityStatsRef.current.push(event.payload);
        return;
      }
      processStatsPayload(event.payload);
    };
    const handleErrorEvent = (event: { payload: SessionErrorPayload }) => {
      if (!mounted || sessionInfoFailedRef.current || !acceptMeta(event.payload)) return;
      onError(event.payload.message);
    };

    const initialize = async () => {
      const listenerResults = await Promise.allSettled([
        listen<AgentEventPayload>("agent-event", handleAgentEvent),
        listen<SessionStatsPayload>("session-stats", handleStatsEvent),
        listen<SessionErrorPayload>("session-error", handleErrorEvent),
      ]);
      const listenerErrors: unknown[] = [];
      for (const result of listenerResults) {
        if (result.status === "fulfilled") unlisteners.push(result.value);
        else listenerErrors.push(result.reason);
      }
      if (!mounted) {
        for (const unlisten of unlisteners) unlisten();
        return;
      }
      if (listenerErrors.length > 0) {
        onError(formatRuntimeError(listenerErrors[0]));
        return;
      }

      const snapshotResults = await Promise.allSettled([
        invoke<MessageView[]>("session_messages"),
        invoke<SessionStatsView>("session_stats"),
        invoke<boolean>("session_running"),
        invoke<SessionInfoView | null>("session_info"),
      ]);
      if (!mounted) return;

      const [messagesResult, statsResult, runningResult, infoResult] = snapshotResults;
      sessionInfoResolvedRef.current = infoResult.status === "fulfilled"
        && (infoResult.value === null || infoResult.value.agentName === agent.name);
      sessionInfoFailedRef.current = !sessionInfoResolvedRef.current;
      if (infoResult.status === "fulfilled" && infoResult.value?.agentName === agent.name) {
        const info = infoResult.value;
        const current = sessionIdentityRef.current;
        if (!current || current.sessionId === info.sessionId) {
          sessionIdentityRef.current = {
            sessionId: info.sessionId,
            runId: Math.max(current?.runId ?? 0, info.runId ?? 0),
          };
          settledRunIdRef.current = info.running ? null : (info.runId ?? null);
          awaitingSessionIdentityRef.current = false;
        }
      }
      const identityEvents = pendingIdentityEventsRef.current;
      const identityStats = pendingIdentityStatsRef.current;
      pendingIdentityEventsRef.current = [];
      pendingIdentityStatsRef.current = [];
      if (sessionInfoResolvedRef.current) {
        identityEvents.forEach(processAgentPayload);
        identityStats.forEach(processStatsPayload);
      }
      const messages = messagesResult.status === "fulfilled" ? messagesResult.value : [];
      const loadedStats = statsResult.status === "fulfilled" ? statsResult.value : null;
      const loadedRunning = runningResult.status === "fulfilled" && runningResult.value;
      dispatch({ type: "hydrate", messages, stats: loadedStats, running: loadedRunning });
      hydrationCompleteRef.current = true;
      const pendingEvents = pendingEventsRef.current;
      const pendingStats = pendingStatsRef.current;
      pendingEventsRef.current = [];
      pendingStatsRef.current = [];
      pendingEvents.forEach((action) => dispatch(action));
      pendingStats.forEach((stats) => dispatch({ type: "stats", stats }));
      const snapshotReady = sessionInfoResolvedRef.current
        && messagesResult.status === "fulfilled"
        && statsResult.status === "fulfilled"
        && runningResult.status === "fulfilled";
      setReady(snapshotReady);
      const failed = snapshotResults.find((result) => result.status === "rejected");
      if (failed?.status === "rejected") onError(formatRuntimeError(failed.reason));
      else if (!sessionInfoResolvedRef.current) onError("无法确认当前会话身份，请返回后重试");
    };

    void initialize();
    return () => {
      mounted = false;
      mountedRef.current = false;
      hydrationCompleteRef.current = false;
      sessionInfoResolvedRef.current = false;
      pendingIdentityEventsRef.current = [];
      pendingIdentityStatsRef.current = [];
      pendingEventsRef.current = [];
      pendingStatsRef.current = [];
      setReady(false);
      if (stopPollTimerRef.current) {
        clearTimeout(stopPollTimerRef.current);
        stopPollTimerRef.current = null;
      }
      for (const unlisten of unlisteners) unlisten();
    };
  }, [agent.name, onError]);

  // 只有用户仍停留在底部时才跟随流式输出，阅读历史时不抢滚动位置。
  useEffect(() => {
    if (!followTailRef.current) return;
    const list = listRef.current;
    if (!list) return;
    const frame = requestAnimationFrame(() => {
      list.scrollTo({ top: list.scrollHeight, behavior: "auto" });
    });
    return () => cancelAnimationFrame(frame);
  }, [entries]);

  useLayoutEffect(() => {
    const textarea = inputRef.current;
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }, [input]);

  useEffect(() => {
    if (!running) {
      setStopping(false);
      if (stopPollTimerRef.current) {
        clearTimeout(stopPollTimerRef.current);
        stopPollTimerRef.current = null;
      }
    }
    onRunningChange(running);
  }, [onRunningChange, running]);

  useEffect(() => {
    return () => {
      if (!runningRef.current) return;
      void invoke("stop_run").catch(() => {});
      onRunningChange(false);
    };
  }, [onRunningChange]);

  const handleScroll = () => {
    const list = listRef.current;
    if (!list) return;
    followTailRef.current = list.scrollHeight - list.scrollTop - list.clientHeight <= 48;
  };

  const send = async () => {
    const text = input.trim();
    if (!ready) return;
    if (!sessionInfoResolvedRef.current) {
      onError("尚未确认当前会话身份，请稍后重试");
      return;
    }
    if (!text || running || sendInFlightRef.current) return;
    const userKey = createEntryKey("user");
    runEpochRef.current += 1;
    awaitingRunIdRef.current = sessionIdentityRef.current?.runId ?? null;
    sendInFlightRef.current = true;
    settledRunIdRef.current = null;
    ignoreEventsUntilNextRunRef.current = false;
    setInput("");
    followTailRef.current = true;
    dispatch({ type: "submit_user", key: userKey, text });
    try {
      await invoke("send_prompt", { agentName: agent.name, prompt: text });
    } catch (error) {
      dispatch({ type: "reject_user", key: userKey });
      onError(formatRuntimeError(error));
    } finally {
      sendInFlightRef.current = false;
    }
  };

  const isStopTarget = (
    epoch: number,
    sessionId: string | undefined,
  ): boolean => {
    if (runEpochRef.current !== epoch) return false;
    const identity = sessionIdentityRef.current;
    if (sessionId !== undefined && identity?.sessionId !== sessionId) return false;
    return true;
  };

  const reconcileStoppedRun = async (
    attempt: number,
    epoch: number,
    sessionId: string | undefined,
  ): Promise<void> => {
    if (!mountedRef.current || !isStopTarget(epoch, sessionId)) return;
    try {
      const stillRunning = await invoke<boolean>("session_running");
      if (!mountedRef.current || !isStopTarget(epoch, sessionId)) return;
      if (stillRunning) {
        if (attempt >= 50) {
          setStopping(false);
          onError("停止请求已发送，但后端仍在运行；请稍后重试");
          return;
        }
        stopPollTimerRef.current = setTimeout(() => {
          void reconcileStoppedRun(attempt + 1, epoch, sessionId);
        }, 100);
        return;
      }

      ignoreEventsUntilNextRunRef.current = true;
      const [messagesResult, statsResult, infoResult] = await Promise.allSettled([
        invoke<MessageView[]>("session_messages"),
        invoke<SessionStatsView>("session_stats"),
        invoke<SessionInfoView | null>("session_info"),
      ]);
      if (!mountedRef.current || !isStopTarget(epoch, sessionId)) return;

      const infoValid = infoResult.status === "fulfilled"
        && infoResult.value !== null
        && infoResult.value.agentName === agent.name;
      if (
        infoResult.status === "fulfilled"
        && infoResult.value !== null
        && infoResult.value.agentName === agent.name
      ) {
        const info = infoResult.value;
        sessionIdentityRef.current = { sessionId: info.sessionId, runId: info.runId };
        awaitingSessionIdentityRef.current = false;
        sessionInfoResolvedRef.current = true;
        sessionInfoFailedRef.current = false;
        settledRunIdRef.current = info.running ? null : (info.runId ?? null);
      } else {
        sessionInfoResolvedRef.current = false;
        sessionInfoFailedRef.current = true;
      }

      if (messagesResult.status === "fulfilled") {
        dispatch({
          type: "hydrate",
          messages: messagesResult.value,
          stats: statsResult.status === "fulfilled" ? statsResult.value : null,
          running: false,
        });
      }
      const snapshotReady = infoValid
        && messagesResult.status === "fulfilled"
        && statsResult.status === "fulfilled";
      setReady(snapshotReady);
      if (!snapshotReady) {
        const failed = [messagesResult, statsResult, infoResult]
          .find((result) => result.status === "rejected");
        onError(
          failed?.status === "rejected"
            ? formatRuntimeError(failed.reason)
            : "停止后无法确认当前会话状态，请返回后重试",
        );
      }
      settledRunIdRef.current = sessionIdentityRef.current?.runId ?? settledRunIdRef.current;
      dispatch({ type: "event", event: { type: "agent_end" }, key: createEntryKey("stop") });
      setStopping(false);
    } catch (error) {
      if (!mountedRef.current) return;
      setStopping(false);
      onError(formatRuntimeError(error));
    }
  };

  const stop = async () => {
    if (!running || stopping) return;
    const epoch = runEpochRef.current;
    const sessionId = sessionIdentityRef.current?.sessionId;
    setStopping(true);
    try {
      await invoke("stop_run");
      void reconcileStoppedRun(0, epoch, sessionId);
    } catch (error) {
      setStopping(false);
      onError(formatRuntimeError(error));
    }
  };

  const newSession = async () => {
    if (!ready || running) return;
    if (!sessionInfoResolvedRef.current) {
      onError("尚未确认当前会话身份，请稍后重试");
      return;
    }
    const previousIdentity = sessionIdentityRef.current;
    const previousAwaiting = awaitingSessionIdentityRef.current;
    const previousSettled = settledRunIdRef.current;
    const previousIgnore = ignoreEventsUntilNextRunRef.current;
    const previousReady = ready;
    const previousInfoResolved = sessionInfoResolvedRef.current;
    const previousInfoFailed = sessionInfoFailedRef.current;
    const previousEpoch = runEpochRef.current;
    const previousAwaitingRunId = awaitingRunIdRef.current;
    if (previousIdentity) blockedSessionIdsRef.current.add(previousIdentity.sessionId);
    sessionIdentityRef.current = null;
    awaitingSessionIdentityRef.current = true;
    sessionInfoResolvedRef.current = false;
    sessionInfoFailedRef.current = false;
    awaitingRunIdRef.current = null;
    settledRunIdRef.current = null;
    ignoreEventsUntilNextRunRef.current = false;
    runEpochRef.current += 1;
    setReady(false);
    const restorePrevious = () => {
      if (previousIdentity) blockedSessionIdsRef.current.delete(previousIdentity.sessionId);
      sessionIdentityRef.current = previousIdentity;
      awaitingSessionIdentityRef.current = previousAwaiting;
      awaitingRunIdRef.current = previousAwaitingRunId;
      settledRunIdRef.current = previousSettled;
      ignoreEventsUntilNextRunRef.current = previousIgnore;
      sessionInfoResolvedRef.current = previousInfoResolved;
      sessionInfoFailedRef.current = previousInfoFailed;
      runEpochRef.current = previousEpoch;
      setReady(previousReady);
    };
    try {
      const accepted = await onNewSession(previousIdentity?.sessionId);
      if (!accepted) {
        restorePrevious();
        return;
      }
      if (!mountedRef.current) return;
      hydrationCompleteRef.current = false;
      pendingIdentityEventsRef.current = [];
      pendingIdentityStatsRef.current = [];
      pendingEventsRef.current = [];
      pendingStatsRef.current = [];
      dispatch({ type: "reset" });
      onSessionReset();
      setInput("");
      followTailRef.current = true;
      const [messagesResult, statsResult, runningResult, infoResult] = await Promise.allSettled([
        invoke<MessageView[]>("session_messages"),
        invoke<SessionStatsView>("session_stats"),
        invoke<boolean>("session_running"),
        invoke<SessionInfoView | null>("session_info"),
      ]);
      if (!mountedRef.current) return;
      sessionInfoResolvedRef.current = infoResult.status === "fulfilled"
        && (infoResult.value === null || infoResult.value.agentName === agent.name);
      sessionInfoFailedRef.current = !sessionInfoResolvedRef.current;
      if (infoResult.status === "fulfilled" && infoResult.value?.agentName === agent.name) {
        sessionIdentityRef.current = {
          sessionId: infoResult.value.sessionId,
          runId: infoResult.value.runId,
        };
        settledRunIdRef.current = infoResult.value.running ? null : (infoResult.value.runId ?? null);
        awaitingSessionIdentityRef.current = false;
      }
      const identityEvents = pendingIdentityEventsRef.current;
      const identityStats = pendingIdentityStatsRef.current;
      pendingIdentityEventsRef.current = [];
      pendingIdentityStatsRef.current = [];
      if (sessionInfoResolvedRef.current) {
        identityEvents.forEach(processAgentPayload);
        identityStats.forEach(processStatsPayload);
      }
      dispatch({
        type: "hydrate",
        messages: messagesResult.status === "fulfilled" ? messagesResult.value : [],
        stats: statsResult.status === "fulfilled" ? statsResult.value : null,
        running: runningResult.status === "fulfilled" && runningResult.value,
      });
      hydrationCompleteRef.current = true;
      const pendingEvents = pendingEventsRef.current;
      const pendingStats = pendingStatsRef.current;
      pendingEventsRef.current = [];
      pendingStatsRef.current = [];
      pendingEvents.forEach((action) => dispatch(action));
      pendingStats.forEach((stats) => dispatch({ type: "stats", stats }));
      const snapshotReady = sessionInfoResolvedRef.current
        && messagesResult.status === "fulfilled"
        && statsResult.status === "fulfilled"
        && runningResult.status === "fulfilled";
      setReady(snapshotReady);
      const failed = [messagesResult, statsResult, runningResult, infoResult]
        .find((result) => result.status === "rejected");
      if (failed?.status === "rejected") onError(formatRuntimeError(failed.reason));
      else if (!sessionInfoResolvedRef.current) onError("无法确认新会话身份，请返回后重试");
      inputRef.current?.focus();
    } catch (error) {
      restorePrevious();
      onError(formatRuntimeError(error));
    }
  };

  const forkSession = async () => {
    if (!ready || running) return;
    const currentId = sessionIdentityRef.current?.sessionId;
    if (!currentId) return;

    try {
      const info = await invoke<SessionInfoView>("fork_session", {
        agentName: agent.name,
        sessionId: currentId,
      });
      sessionIdentityRef.current = {
        sessionId: info.sessionId,
        runId: info.runId,
      };
      settledRunIdRef.current = null;
      onSessionReset();
      const messages = await invoke<MessageView[]>("session_messages");
      const loadedStats = await invoke<SessionStatsView>("session_stats").catch(() => null);
      dispatch({ type: "hydrate", messages, stats: loadedStats, running: false });
    } catch (err) {
      onError(formatRuntimeError(err));
    }
  };

  const statsBits: string[] = [];
  if (stats) {
    if (stats.avgTps != null) statsBits.push(`${stats.avgTps.toFixed(1)} tok/s`);
    if (stats.cacheHitPct != null) statsBits.push(`缓存命中 ${stats.cacheHitPct.toFixed(0)}%`);
    if (stats.contextPercent != null) {
      statsBits.push(`上下文 ${stats.contextUsed}/${stats.contextMax}（${stats.contextPercent}%）`);
    }
    statsBits.push(`${stats.calls} 次调用`);
  }

  return (
    <div className="chat">
      <header className="chat-header">
        <button type="button" className="ghost" onClick={onBack}>
          ← 返回
        </button>
        <div className="chat-title">
          <span className="mono">{agent.name}</span>
          {statsBits.length > 0 && (
            <span className="badge neutral chat-stats" title="最近 10 次调用的滚动统计">
              {statsBits.join(" · ")}
            </span>
          )}
        </div>
        <div className="chat-actions">
          <button
            type="button"
            className="ghost"
            onClick={forkSession}
            disabled={!ready || running || !sessionIdentityRef.current?.sessionId}
            title="从当前对话节点分叉出新会话"
          >
            分叉
          </button>
          <button type="button" className="ghost" onClick={newSession} disabled={!ready || running}>
            新会话
          </button>
        </div>
      </header>

      <div
        className="chat-list"
        ref={listRef}
        onScroll={handleScroll}
        aria-live={running ? "polite" : undefined}
      >
        {entries.length === 0 && (
          <div className="chat-welcome">
            <p>
              与 <span className="mono">{agent.name}</span> 对话。会话记录将写入
              <span className="mono"> sessions/*.jsonl</span>。
            </p>
          </div>
        )}
        {entries.map((entry) => (
          <div key={entry.key} className={`chat-entry role-${entry.role}`}>
            <div className="chat-role">
              {entry.role === "user" ? "你" : entry.role === "assistant" ? agent.name : "工具"}
            </div>
            <div className={`chat-bubble ${entry.isError ? "is-error" : ""}`}>
              {entry.role === "assistant" ? (
                <>
                  {entry.thinking && (
                    <details className="chat-thinking" open={entry.streaming && !entry.text}>
                      <summary className="thinking-summary">
                        <span className="thinking-icon">💭</span>
                        <span className="thinking-label">
                          {entry.streaming && !entry.text
                            ? "思考中…"
                            : `深度思考（${entry.thinking.length} 字）`}
                        </span>
                      </summary>
                      <pre className="thinking-body mono">{entry.thinking}</pre>
                    </details>
                  )}
                  <Markdown text={entry.text || (entry.streaming ? "…" : "")} />
                </>
              ) : entry.role === "toolResult" ? (
                <ToolResultCard entry={entry} />
              ) : (
                <pre className="chat-text">{entry.text || (entry.streaming ? "…" : "")}</pre>
              )}
              {entry.role === "assistant" && !entry.streaming && !entry.status && (
                <div className="chat-foot">
                  {assistantFooter({
                    role: "assistant",
                    content: entry.text,
                    usage: entry.usage,
                    durationMs: entry.durationMs,
                  })}
                </div>
              )}
              {entry.streaming && <div className="chat-cursor">▍</div>}
            </div>
          </div>
        ))}
      </div>

      <footer className="chat-input">
        <textarea
          ref={inputRef}
          value={input}
          onChange={(event) => setInput(event.target.value)}
          placeholder={ready ? (running ? "Agent 正在运行…" : "输入消息，Enter 发送（Shift+Enter 换行）") : "正在连接 Agent…"}
          disabled={!ready || running}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
            compositionEndedAtRef.current = Date.now();
          }}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.shiftKey) return;
            const native = event.nativeEvent;
            const composing = composingRef.current
              || native.isComposing
              || native.keyCode === 229
              || Date.now() - compositionEndedAtRef.current < 100;
            if (composing) return;
            event.preventDefault();
            void send();
          }}
        />
        {running ? (
          <button type="button" className="ghost stop" onClick={stop} disabled={stopping}>
            {stopping ? "■ 停止中…" : "■ 停止"}
          </button>
        ) : (
          <button type="button" className="primary" disabled={!ready || !sessionInfoResolvedRef.current || !input.trim()} onClick={() => void send()}>
            发送
          </button>
        )}
      </footer>
    </div>
  );
}

function ToolResultCard({ entry }: { entry: import("./chat-runtime").ChatEntry }) {
  const [open, setOpen] = useState(!entry.text.includes("\n") || entry.isError || entry.toolRunning);
  const toolName = entry.toolName ?? "tool";
  const statusLabel = entry.toolRunning
    ? "运行中"
    : entry.isError
      ? "失败"
      : "完成";
  const statusClass = entry.toolRunning ? "running" : entry.isError ? "error" : "success";

  let commandStr: string | null = null;
  let pathStr: string | null = null;
  if (entry.toolArgs && typeof entry.toolArgs === "object") {
    const args = entry.toolArgs as Record<string, unknown>;
    if (typeof args.command === "string") commandStr = args.command;
    if (typeof args.path === "string") pathStr = args.path;
  }

  const details = entry.toolDetails as { diff?: string } | undefined;
  const hasDiff = typeof details?.diff === "string" && details.diff.trim().length > 0;

  return (
    <div className={`tool-card ${statusClass}`}>
      <div className="tool-card-head" onClick={() => setOpen(!open)}>
        <span className="tool-card-name mono">
          <span className="tool-icon">⚙</span> {toolName}
        </span>
        {commandStr && <span className="tool-summary-cmd mono" title={commandStr}>$ {commandStr}</span>}
        {!commandStr && pathStr && <span className="tool-summary-cmd mono" title={pathStr}>{pathStr}</span>}
        <span className={`badge tool-status ${statusClass}`}>{statusLabel}</span>
        <span className="tool-toggle-btn">
          {open ? "收起 ▲" : "详情 ▼"}
        </span>
      </div>

      {open && (
        <div className="tool-card-body">
          {entry.toolArgs != null && (
            <div className="tool-section">
              <div className="tool-section-label">参数</div>
              <pre className="tool-code mono">
                {JSON.stringify(entry.toolArgs, null, 2)}
              </pre>
            </div>
          )}


          <div className="tool-section">
            <div className="tool-section-label">输出</div>
            {hasDiff ? (
              <pre className="tool-diff mono">
                {details!.diff!.split("\n").map((line, idx) => {
                  let lineClass = "";
                  if (line.startsWith("+")) lineClass = "diff-add";
                  else if (line.startsWith("-")) lineClass = "diff-del";
                  else if (line.startsWith("@@")) lineClass = "diff-hunk";
                  return (
                    <div key={idx} className={lineClass}>
                      {line}
                    </div>
                  );
                })}
              </pre>
            ) : (
              <pre className="tool-output-text mono">
                {entry.text || (entry.toolRunning ? "执行中…" : "（无输出）")}
              </pre>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

