import { useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { Markdown } from "./Markdown";
import { invoke, listen } from "./platform";
import { ScreenTabs } from "./ScreenTabs";
import { IconBack, IconFork, IconGrid, IconPlus, IconSend, IconStop } from "./icons";
import {
  catalogSourceLabel,
  findCatalogModel,
  matchProvider,
  resolveContextWindow,
  resolveMaxTokens,
  type ModelCatalog,
} from "./catalog";
import { loadCatalog, peekCatalog } from "./catalog-client";
import { ModelPicker } from "./ModelPicker";
import { ChoiceSelect } from "./Select";
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
  type ChatEntry,
  type MessageView,
  type SessionErrorPayload,
  type SessionEventMeta,
  type SessionStatsPayload,
} from "./chat-runtime";
import type { AgentDefinition, ModelConfig, ProviderConfig, SessionInfoView, SessionStatsView } from "./types";

export type { MessageView } from "./chat-runtime";

const WRITE_TOOLS = new Set(["write", "edit"]);

interface ChatViewProps {
  agent: AgentDefinition;
  providers: ProviderConfig[];
  blockedSessionIds: string[];
  onBack: () => void;
  onShowDetail: () => void;
  onError: (msg: string) => void;
  onNewSession: (previousSessionId?: string) => Promise<boolean>;
  onRunningChange: (running: boolean) => void;
  /** 新会话：传 null 表示无会话；分叉/重建后传新的会话信息以同步侧栏高亮。 */
  onSessionReset: (info?: SessionInfoView | null) => void;
}

export default function ChatView({
  agent,
  providers,
  blockedSessionIds,
  onBack,
  onShowDetail,
  onError,
  onNewSession,
  onRunningChange,
  onSessionReset,
}: ChatViewProps) {
  const [state, dispatch] = useReducer(chatReducer, INITIAL_CHAT_STATE);
  const { entries, stats, running } = state;
  const runningRef = useRef(running);
  runningRef.current = running;
  const [ready, setReady] = useState(false);
  const [input, setInput] = useState("");
  const [stopping, setStopping] = useState(false);
  const [sessionModel, setSessionModel] = useState<ModelConfig | null>(agent.provider ?? null);
  const [isCustomModel, setIsCustomModel] = useState(false);
  const [modelModalOpen, setModelModalOpen] = useState(false);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [inspectorOpen, setInspectorOpen] = useState(
    () => typeof window === "undefined" || window.innerWidth > 1100,
  );
  const [narrow, setNarrow] = useState(
    () => typeof window !== "undefined" && window.innerWidth <= 1100,
  );
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
        setSessionId(info.sessionId);
        if (info.model !== undefined) {
          setSessionModel(info.model ?? agent.provider ?? null);
          setIsCustomModel(Boolean(info.isCustomModel));
        }
      } else if (infoResult.status === "fulfilled" && infoResult.value === null) {
        setSessionModel(agent.provider ?? null);
        setIsCustomModel(false);
        setSessionId(null);
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

  useEffect(() => {
    if (!isCustomModel) {
      setSessionModel(agent.provider ?? null);
    }
  }, [agent.provider, isCustomModel]);

  // 检查器在窄屏是抽屉：跨过断点时对齐默认值（宽屏展开 / 窄屏收起），
  // 不覆盖用户在同一布局下的手动切换。
  useEffect(() => {
    const onResize = () => setNarrow(window.innerWidth <= 1100);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    setInspectorOpen(!narrow);
  }, [narrow]);

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
      await invoke("send_prompt", {
        agentName: agent.name,
        prompt: text,
        model: isCustomModel ? sessionModel : null,
      });
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
        setSessionId(info.sessionId);
        if (info.model !== undefined) {
          setSessionModel(info.model ?? agent.provider ?? null);
          setIsCustomModel(Boolean(info.isCustomModel));
        }
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
        setSessionId(infoResult.value.sessionId);
        if (infoResult.value.model !== undefined) {
          setSessionModel(infoResult.value.model ?? agent.provider ?? null);
          setIsCustomModel(Boolean(infoResult.value.isCustomModel));
        }
      } else {
        setSessionModel(agent.provider ?? null);
        setIsCustomModel(false);
        setSessionId(null);
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
      setSessionId(info.sessionId);
      if (info.model !== undefined) {
        setSessionModel(info.model ?? agent.provider ?? null);
        setIsCustomModel(Boolean(info.isCustomModel));
      }
      onSessionReset(info);
      const messages = await invoke<MessageView[]>("session_messages");
      const loadedStats = await invoke<SessionStatsView>("session_stats").catch(() => null);
      dispatch({ type: "hydrate", messages, stats: loadedStats, running: false });
    } catch (err) {
      onError(formatRuntimeError(err));
    }
  };

  const handleModelChange = async (target: { isCustom: boolean; model: ModelConfig | null }) => {
    const modelToSet = target.isCustom ? target.model : null;
    const hasActiveSession = Boolean(sessionIdentityRef.current?.sessionId);
    if (hasActiveSession) {
      try {
        await invoke("set_session_model", { model: modelToSet });
      } catch (err) {
        throw new Error(formatRuntimeError(err));
      }
    }
    setSessionModel(target.isCustom ? target.model : (agent.provider ?? null));
    setIsCustomModel(target.isCustom);
    setModelModalOpen(false);
  };

  return (
    <div className={`chat-shell${inspectorOpen ? " inspector-open" : ""}`}>
      <div className="chat">
        <div className="screen-bar">
          <button type="button" className="icon-btn" title="返回" aria-label="返回" onClick={onBack}>
            <IconBack />
          </button>
          <span className="crumb" title={`~/.pipi/agents/${agent.name}/sessions/${sessionId ?? ""}`}>
            ~/.pipi/agents/<b>{agent.name}</b>/sessions/{sessionId ? <b>{sessionId}</b> : "…"}
          </span>
          <ScreenTabs
            active="chat"
            onSelect={(view) => {
              if (view === "detail") onShowDetail();
            }}
          />
          <span className="spacer" />
          <button
            type="button"
            className="model-pill"
            onClick={() => setModelModalOpen(true)}
            disabled={running}
            title={running ? "Agent 运行中不可切换模型" : "点击切换当前会话的模型"}
          >
            <span className="model-pill-name mono">{sessionModel?.id || "未配置模型"}</span>
            <span className="model-pill-tag">{isCustomModel ? "自定义" : "默认"}</span>
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={forkSession}
            disabled={!ready || running || !sessionIdentityRef.current?.sessionId}
            title="从当前对话节点分叉出新会话"
            aria-label="分叉会话"
          >
            <IconFork />
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={newSession}
            disabled={!ready || running}
            title="新会话"
            aria-label="新建会话"
          >
            <IconPlus />
          </button>
          <button
            type="button"
            className={`icon-btn${inspectorOpen ? " active" : ""}`}
            onClick={() => setInspectorOpen((open) => !open)}
            title="统计 / 检查器"
            aria-label="切换检查器"
            aria-expanded={inspectorOpen}
            aria-controls="session-inspector"
          >
            <IconGrid />
          </button>
        </div>

        <div
          className="log"
          ref={listRef}
          onScroll={handleScroll}
          aria-live={running ? "polite" : undefined}
        >
          {entries.length === 0 && (
            <div className="chat-welcome">
              与 <span className="mono">{agent.name}</span> 对话。会话记录将写入
              <span className="mono"> sessions/*.jsonl</span>。
            </div>
          )}
          {entries.map((entry) => {
            const isUser = entry.role === "user";
            const isTool = entry.role === "toolResult";
            const tagClass = isUser ? "user" : isTool ? "tool" : entry.isError ? "error" : "";
            const tagText = isUser ? "you" : isTool ? "tool" : "agent";
            const footer = !isTool && !isUser && !entry.streaming && !entry.status
              ? assistantFooter({
                  role: "assistant",
                  content: entry.text,
                  usage: entry.usage,
                  durationMs: entry.durationMs,
                })
              : null;
            return (
              <div key={entry.key} className={`row${isTool ? " alt" : ""}${isUser ? " user-row" : ""}`}>
                <div className="gut">
                  {entry.timestamp ? <span className="time">{formatClock(entry.timestamp)}</span> : null}
                  <span className={`role-tag ${tagClass}`}>{tagText}</span>
                </div>
                <div className="content">
                  {isUser ? (
                    <div className="user-text">{entry.text}</div>
                  ) : isTool ? (
                    <ToolResultCard entry={entry} />
                  ) : (
                    <>
                      {entry.thinking && (
                        <ThinkingFold
                          thinking={entry.thinking}
                          streaming={Boolean(entry.streaming)}
                          hasText={Boolean(entry.text)}
                        />
                      )}
                      <Markdown text={entry.text || (entry.streaming ? "…" : "")} />
                      {entry.streaming && <span className="cursor" aria-hidden="true" />}
                      {footer && <div className="hint">{footer}</div>}
                    </>
                  )}
                </div>
              </div>
            );
          })}
        </div>

        <div className="composer">
          <div className="composer-box">
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
            <div className="composer-bar">
              <span className="hint">ENTER 发送 · SHIFT+ENTER 换行</span>
              <span className="spacer" />
              {running ? (
                <button type="button" className="btn stop" onClick={stop} disabled={stopping}>
                  <IconStop />
                  {stopping ? "停止中…" : "停止"}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn primary"
                  disabled={!ready || !sessionInfoResolvedRef.current || !input.trim()}
                  onClick={() => void send()}
                >
                  发送
                  <IconSend />
                </button>
              )}
            </div>
          </div>
        </div>
      </div>

      <Inspector
        agent={agent}
        sessionId={sessionId}
        entries={entries}
        stats={stats}
        running={running}
        sessionModel={sessionModel}
        isCustomModel={isCustomModel}
        blockedCount={blockedSessionIds.length}
      />

      {narrow && inspectorOpen && (
        <button
          type="button"
          className="inspector-backdrop"
          aria-label="关闭检查器"
          onClick={() => setInspectorOpen(false)}
        />
      )}

      {modelModalOpen && (
        <ModelSelectModal
          isOpen={modelModalOpen}
          onClose={() => setModelModalOpen(false)}
          onConfirm={handleModelChange}
          currentModel={sessionModel}
          isCustomModel={isCustomModel}
          agentDefaultModel={agent.provider}
          providers={providers}
        />
      )}
    </div>
  );
}

function formatClock(timestamp: number): string {
  const date = new Date(timestamp);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function formatTokens(value: number | undefined): string {
  if (value == null) return "—";
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

function ThinkingFold({
  thinking,
  streaming,
  hasText,
}: {
  thinking: string;
  streaming: boolean;
  hasText: boolean;
}) {
  const autoOpen = streaming && !hasText;
  const [open, setOpen] = useState(autoOpen);

  useEffect(() => {
    if (autoOpen) setOpen(true);
  }, [autoOpen]);

  return (
    <>
      <button
        type="button"
        className={`fold${open ? " open" : ""}`}
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <span className="k">THINKING</span>
        <span>{streaming && !hasText ? "思考中…" : `深度思考（${thinking.length} 字）`}</span>
        <span className="caret">▶</span>
      </button>
      {open && <pre className="thinking-body">{thinking}</pre>}
    </>
  );
}

function ToolResultCard({ entry }: { entry: ChatEntry }) {
  const [open, setOpen] = useState(!entry.text.includes("\n") || Boolean(entry.isError) || Boolean(entry.toolRunning));
  const toolName = entry.toolName ?? "tool";
  const running = Boolean(entry.toolRunning);
  const failed = Boolean(entry.isError);

  let commandStr: string | null = null;
  let pathStr: string | null = null;
  if (entry.toolArgs && typeof entry.toolArgs === "object") {
    const args = entry.toolArgs as Record<string, unknown>;
    if (typeof args.command === "string") commandStr = args.command;
    if (typeof args.path === "string") pathStr = args.path;
  }

  const details = entry.toolDetails as { diff?: string } | undefined;
  const hasDiff = typeof details?.diff === "string" && details.diff.trim().length > 0;
  const argsText = entry.toolArgs != null ? JSON.stringify(entry.toolArgs, null, 2) : null;
  const summary = commandStr ?? pathStr;

  return (
    <div className={`tool${running ? " running" : failed ? " error" : ""}`}>
      <button
        type="button"
        className="tool-head"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <span className="tool-name">{toolName}</span>
        {summary && (
          <span className="tool-cmd" title={summary}>
            {summary}
          </span>
        )}
        <span className="tool-meta">
          {running ? (
            <span className="run">● 运行中</span>
          ) : failed ? (
            <span className="fail">✕ 失败</span>
          ) : (
            <span className="ok">✓ 完成</span>
          )}
          <span>{open ? "▾" : "▸"}</span>
        </span>
      </button>

      {open && (
        <div className="tool-body">
          {argsText && (
            <div className="tpane">
              <div className="plabel">参数</div>
              <pre className="code">{argsText}</pre>
            </div>
          )}
          <div className={`tpane${argsText ? "" : " full"}`}>
            <div className="plabel">输出</div>
            {hasDiff ? (
              <pre className="code">
                {details!.diff!.split("\n").map((line, index) => {
                  let lineClass = "ln-ctx";
                  if (line.startsWith("+++") || line.startsWith("---")) lineClass = "ln-ctx";
                  else if (line.startsWith("+")) lineClass = "ln-add";
                  else if (line.startsWith("-")) lineClass = "ln-del";
                  else if (line.startsWith("@@")) lineClass = "ln-hunk";
                  return (
                    <span key={index} className={lineClass}>
                      {line}
                    </span>
                  );
                })}
              </pre>
            ) : (
              <pre className="code">{entry.text || (running ? "执行中…" : "（无输出）")}</pre>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ============ 右侧检查器 ============

type InspectorTab = "stats" | "state" | "files";

interface InspectorProps {
  agent: AgentDefinition;
  sessionId: string | null;
  entries: ChatEntry[];
  stats: SessionStatsView | null;
  running: boolean;
  sessionModel: ModelConfig | null;
  isCustomModel: boolean;
  blockedCount: number;
}

const INSPECTOR_TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: "stats", label: "会话统计" },
  { id: "state", label: "当前状态" },
  { id: "files", label: "文件变更" },
];

const BASH_MODE_TEXT: Record<string, string> = {
  allowAll: "全部允许",
  allowlist: "白名单",
  denylist: "黑名单",
};

function Inspector({
  agent,
  sessionId,
  entries,
  stats,
  running,
  sessionModel,
  isCustomModel,
  blockedCount,
}: InspectorProps) {
  const [tab, setTab] = useState<InspectorTab>("stats");

  const lastTool = useMemo(() => {
    for (let index = entries.length - 1; index >= 0; index -= 1) {
      if (entries[index].role === "toolResult") return entries[index];
    }
    return null;
  }, [entries]);

  // 「当前工具」只在真的有工具在跑时才算数；否则只显示最近一次工具。
  const runningTool = useMemo(() => {
    for (let index = entries.length - 1; index >= 0; index -= 1) {
      const entry = entries[index];
      if (entry.role === "toolResult" && entry.toolRunning) return entry;
    }
    return null;
  }, [entries]);

  const handleTabKeys = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const index = INSPECTOR_TABS.findIndex((item) => item.id === tab);
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const next = INSPECTOR_TABS[(index + delta + INSPECTOR_TABS.length) % INSPECTOR_TABS.length];
    setTab(next.id);
    document.getElementById(`itab-${next.id}`)?.focus();
  };

  const toolSummary = useMemo(() => {
    const map = new Map<string, { count: number; errors: number; running: number }>();
    for (const entry of entries) {
      if (entry.role !== "toolResult") continue;
      const name = entry.toolName ?? "tool";
      const item = map.get(name) ?? { count: 0, errors: 0, running: 0 };
      item.count += 1;
      if (entry.toolRunning) item.running += 1;
      else if (entry.isError || entry.status === "tool-error") item.errors += 1;
      map.set(name, item);
    }
    return [...map.entries()]
      .map(([name, value]) => ({ name, ...value }))
      .sort((left, right) => right.count - left.count);
  }, [entries]);

  const fileChanges = useMemo(() => {
    const map = new Map<string, { path: string; add: number; del: number; calls: number; errors: number }>();
    for (const entry of entries) {
      if (entry.role !== "toolResult") continue;
      const name = entry.toolName ?? "";
      if (!WRITE_TOOLS.has(name)) continue;
      const args = (entry.toolArgs ?? {}) as { path?: unknown };
      const path = typeof args.path === "string" && args.path.trim()
        ? args.path
        : `（未记录路径的 ${name} 调用）`;
      const diff = (entry.toolDetails as { diff?: unknown } | undefined)?.diff;
      let add = 0;
      let del = 0;
      if (typeof diff === "string") {
        for (const line of diff.split("\n")) {
          if (line.startsWith("+++") || line.startsWith("---")) continue;
          if (line.startsWith("+")) add += 1;
          else if (line.startsWith("-")) del += 1;
        }
      }
      const item = map.get(path) ?? { path, add: 0, del: 0, calls: 0, errors: 0 };
      item.calls += 1;
      item.add += add;
      item.del += del;
      if (entry.isError) item.errors += 1;
      map.set(path, item);
    }
    return [...map.values()].reverse();
  }, [entries]);

  const writeCalls = fileChanges.reduce((total, item) => total + item.calls, 0);
  const maxToolCount = toolSummary.reduce((max, item) => Math.max(max, item.count), 0);
  const workspace = agent.workspace ?? `~/.pipi/agents/${agent.name}/workspace`;
  const currentToolText = running
    ? (runningTool ? `${runningTool.toolName ?? "tool"} · 运行中` : "模型调用中")
    : (lastTool ? `上次工具 ${lastTool.toolName ?? "tool"}` : "尚无工具调用");

  return (
    <aside className="inspector" id="session-inspector" aria-label="会话检查器">
      <div className="itabs" role="tablist" aria-label="检查器视图" onKeyDown={handleTabKeys}>
        {INSPECTOR_TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            id={`itab-${item.id}`}
            aria-selected={tab === item.id}
            aria-controls={`ipanel-${item.id}`}
            tabIndex={tab === item.id ? 0 : -1}
            className={`itab${tab === item.id ? " active" : ""}`}
            onClick={() => setTab(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div className="ipanels">
        <div
          className={`ipanel${tab === "stats" ? " active" : ""}`}
          id="ipanel-stats"
          role="tabpanel"
          aria-labelledby="itab-stats"
        >
            <div className="ip">
              <h4>当前上下文</h4>
              {stats?.contextPercent != null || stats?.cacheHitPct != null ? (
                <>
                  {stats?.contextPercent != null && (
                    <div className="metric">
                      <span className="m-k">上下文占用</span>
                      <span className="m-v">
                        {formatTokens(stats.contextUsed)} / {formatTokens(stats.contextMax)}
                      </span>
                      <span className="track" title="最近一次请求的 prompt 用量 / 上下文窗口">
                        <i style={{ width: `${Math.min(100, Math.max(0, stats.contextPercent))}%` }} />
                      </span>
                    </div>
                  )}
                  {stats?.cacheHitPct != null && (
                    <div className="metric">
                      <span className="m-k">缓存命中</span>
                      <span className="m-v">{stats.cacheHitPct.toFixed(0)}%</span>
                      <span className="track" title="最近一次调用：cache_read / prompt">
                        <i style={{ width: `${Math.min(100, Math.max(0, stats.cacheHitPct))}%` }} />
                      </span>
                    </div>
                  )}
                </>
              ) : (
                <div className="ip-note">暂无统计：本次会话还没有产生调用。</div>
              )}
            </div>

            <div className="ip">
              <h4>近 10 次调用</h4>
              {stats && (stats.avgTps != null || stats.avgLatencyS != null) ? (
                <>
                  {stats.avgTps != null && (
                    <div className="metric">
                      <span className="m-k">输出速度</span>
                      <span className="m-v">{stats.avgTps.toFixed(1)} tok/s</span>
                      <span className="track" title="滚动平均；进度条以 100 tok/s 为满量程">
                        <i
                          className="warm"
                          style={{ width: `${Math.min(100, Math.max(0, stats.avgTps))}%` }}
                        />
                      </span>
                    </div>
                  )}
                  {stats.avgLatencyS != null && (
                    <div className="metric">
                      <span className="m-k">平均延迟</span>
                      <span className="m-v">{stats.avgLatencyS.toFixed(2)}s</span>
                    </div>
                  )}
                </>
              ) : (
                <div className="ip-note">有效调用不足，暂不计算窗口指标。</div>
              )}
            </div>

            <div className="ip">
              <h4>累计</h4>
              <div className="metric">
                <span className="m-k">调用次数</span>
                <span className="m-v">{stats?.calls ?? 0}</span>
              </div>
              <div className="metric">
                <span className="m-k">输入 / 输出</span>
                <span className="m-v dim">
                  {formatTokens(stats?.input)} / {formatTokens(stats?.output)}
                </span>
              </div>
            </div>

            <div className="ip">
              <h4>工具调用</h4>
              {toolSummary.length > 0 ? (
                <div className="tl">
                  {toolSummary.map((item) => (
                    <div className="tl-row" key={item.name}>
                      <span className="n" title={item.name}>
                        {item.name}
                      </span>
                      <span className="b">
                        <i
                          style={{
                            width: `${maxToolCount > 0 ? Math.max(6, (item.count / maxToolCount) * 100) : 0}%`,
                          }}
                        />
                      </span>
                      <span className={`d${item.errors > 0 ? " fail" : ""}`}>
                        {item.count} 次{item.errors > 0 ? ` · ${item.errors} 失败` : ""}
                      </span>
                    </div>
                  ))}
                </div>
              ) : (
                <div className="ip-note">本次会话尚未调用工具。</div>
              )}
            </div>
        </div>

        <div
          className={`ipanel${tab === "state" ? " active" : ""}`}
          id="ipanel-state"
          role="tabpanel"
          aria-labelledby="itab-state"
        >
            <div className="ip">
              <h4>运行状态</h4>
              <div className={`state-line${running ? "" : " idle"}`}>
                <span className="dot" aria-hidden="true" />
                {running ? "运行中" : "空闲"}
                <span className="sub">{currentToolText}</span>
              </div>
              <div className="kv-row">
                <span className="k">{runningTool ? "当前工具" : "最近工具"}</span>
                <span className="v">
                  {runningTool || lastTool ? (
                    <>
                      <span className="mono" style={{ color: "var(--accent-text)" }}>
                        {(runningTool ?? lastTool)!.toolName ?? "tool"}
                      </span>{" "}
                      <span className="dim">
                        {runningTool ? "运行中" : lastTool!.isError ? "失败" : "完成"}
                      </span>
                    </>
                  ) : (
                    <span className="dim">—</span>
                  )}
                </span>
              </div>
              <div className="kv-row">
                <span className="k">消息条数</span>
                <span className="v mono">{entries.length}</span>
              </div>
              <div className="kv-row">
                <span className="k">调用次数</span>
                <span className="v mono">{stats?.calls ?? 0}</span>
              </div>
              <div className="kv-row">
                <span className="k">阻塞会话</span>
                <span className="v mono dim">{blockedCount}</span>
              </div>
            </div>

            <div className="ip">
              <h4>会话</h4>
              <div className="kv-row">
                <span className="k">模型</span>
                <span className="v mono">
                  {sessionModel?.id || "未配置"}
                  {sessionModel && <span className="dim"> {isCustomModel ? "自定义" : "默认"}</span>}
                </span>
              </div>
              <div className="kv-row">
                <span className="k">会话 ID</span>
                <span className="v mono">{sessionId ?? "（新会话）"}</span>
              </div>
              <div className="kv-row">
                <span className="k">上下文</span>
                <span className="v mono">
                  {stats?.contextPercent != null ? (
                    <>
                      {formatTokens(stats.contextUsed)} / {formatTokens(stats.contextMax)}{" "}
                      <span className="dim">{stats.contextPercent}%</span>
                    </>
                  ) : (
                    <span className="dim">—</span>
                  )}
                </span>
              </div>
              <div className="kv-row">
                <span className="k">会话文件</span>
                <span className="v mono dim">
                  {sessionId ? `~/.pipi/agents/${agent.name}/sessions/` : "—"}
                </span>
              </div>
            </div>

            <div className="ip">
              <h4>约束</h4>
              <div className="kv-row">
                <span className="k">沙箱</span>
                <span className="v">
                  <span className="tag ok">{agent.permissions.sandbox}</span>
                </span>
              </div>
              <div className="kv-row">
                <span className="k">bash</span>
                <span className="v mono">
                  {BASH_MODE_TEXT[agent.permissions.bash.mode] ?? agent.permissions.bash.mode}
                  <span className="dim">
                    {agent.permissions.bash.mode !== "allowAll"
                      ? ` · ${agent.permissions.bash.commands.length} 条`
                      : ""}
                  </span>
                </span>
              </div>
              <div className="kv-row">
                <span className="k">工作目录</span>
                <span className="v mono">{workspace}</span>
              </div>
              <div className="kv-row">
                <span className="k">工具</span>
                <span className="v mono dim">
                  {agent.permissions.tools.length ? agent.permissions.tools.join(" · ") : "（无）"}
                </span>
              </div>
            </div>
        </div>

        <div
          className={`ipanel${tab === "files" ? " active" : ""}`}
          id="ipanel-files"
          role="tabpanel"
          aria-labelledby="itab-files"
        >
            <div className="ip">
              <h4>本次会话改动</h4>
              {fileChanges.length > 0 ? (
                <div className="chg">
                  {fileChanges.map((item) => (
                    <div className="chg-row" key={item.path} title={item.path}>
                      <span className="p">{item.path}</span>
                      <span className="add">+{item.add}</span>
                      <span className="del">−{item.del}</span>
                    </div>
                  ))}
                </div>
              ) : (
                <div className="ip-note">本次会话还没有 write / edit 调用。</div>
              )}
            </div>

            <div className="ip">
              <h4>变更统计</h4>
              <div className="metric">
                <span className="m-k">文件</span>
                <span className="m-v">{fileChanges.length}</span>
              </div>
              <div className="metric">
                <span className="m-k">新增 / 删除</span>
                <span className="m-v dim">
                  +{fileChanges.reduce((total, item) => total + item.add, 0)} / −
                  {fileChanges.reduce((total, item) => total + item.del, 0)}
                </span>
              </div>
              <div className="metric">
                <span className="m-k">写入调用</span>
                <span className="m-v dim">{writeCalls}</span>
              </div>
            </div>
        </div>
      </div>
    </aside>
  );
}

// ============ 会话模型选择 ============

interface ModelSelectModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: (target: { isCustom: boolean; model: ModelConfig | null }) => Promise<void>;
  currentModel: ModelConfig | null;
  isCustomModel: boolean;
  agentDefaultModel: ModelConfig | null;
  providers: ProviderConfig[];
}

function ModelSelectModal({
  isOpen,
  onClose,
  onConfirm,
  currentModel,
  isCustomModel,
  agentDefaultModel,
  providers,
}: ModelSelectModalProps) {
  const [mode, setMode] = useState<"default" | "custom">(isCustomModel ? "custom" : "default");

  const initialProvider = providers.find((p) => {
    if (isCustomModel && currentModel) {
      return p.api === currentModel.api && (p.baseUrl || "") === (currentModel.baseUrl || "");
    }
    if (agentDefaultModel) {
      return p.api === agentDefaultModel.api && (p.baseUrl || "") === (agentDefaultModel.baseUrl || "");
    }
    return false;
  }) ?? providers[0];

  const [selectedProviderId, setSelectedProviderId] = useState<string>(initialProvider?.id || "");
  const [modelId, setModelId] = useState<string>(
    isCustomModel && currentModel ? currentModel.id : ""
  );
  const [maxTokens, setMaxTokens] = useState<number>(
    isCustomModel && currentModel ? currentModel.maxTokens : 8192
  );
  const [contextWindow, setContextWindow] = useState<number>(
    isCustomModel && currentModel ? currentModel.contextWindow : 0
  );
  const [saving, setSaving] = useState(false);
  const [modalError, setModalError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<ModelCatalog | null>(peekCatalog());

  useEffect(() => {
    if (peekCatalog()) return;
    let alive = true;
    loadCatalog()
      .then((next) => {
        if (alive) setCatalog(next);
      })
      .catch(() => {
        // 目录拉不到不影响切换模型：ModelPicker 会退化成手填
      });
    return () => {
      alive = false;
    };
  }, []);

  // 当前选中的供应商在目录里的条目（按「协议 + 端点」反查，口径与会话绑定一致）
  const activeProvider = providers.find((p) => p.id === selectedProviderId);
  const entry = matchProvider(catalog, activeProvider?.api ?? "", activeProvider?.baseUrl ?? "");

  if (!isOpen) return null;

  // 切换供应商必须清掉上一个供应商残留的模型 ID 与限额，
  // 否则会保存出「B 的端点 + A 的模型/上限」这种静默错配。
  const handleProviderChange = (nextProviderId: string) => {
    setSelectedProviderId(nextProviderId);
    setModelId("");
    setMaxTokens(8192);
    setContextWindow(0);
  };
  const pickedModel = findCatalogModel(entry, modelId);

  const handleSave = async () => {
    setModalError(null);
    setSaving(true);
    try {
      if (mode === "default") {
        await onConfirm({ isCustom: false, model: agentDefaultModel });
      } else {
        const trimmed = modelId.trim();
        if (!trimmed) {
          setModalError("请输入模型 ID（如 claude-3-7-sonnet-20250219）");
          setSaving(false);
          return;
        }
        const provider = providers.find((p) => p.id === selectedProviderId);
        if (!provider) {
          setModalError("请选择有效的供应商配置");
          setSaving(false);
          return;
        }
        const targetModel: ModelConfig = {
          id: trimmed,
          name: trimmed,
          api: provider.api,
          baseUrl: provider.baseUrl || "",
          maxTokens: maxTokens > 0 ? maxTokens : 8192,
          contextWindow: contextWindow > 0 ? contextWindow : 0,
        };
        await onConfirm({ isCustom: true, model: targetModel });
      }
    } catch (err: unknown) {
      setModalError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget && !saving) onClose();
      }}
    >
      <div className="modal model-select-modal">
        <div className="modal-header">
          <h2>选择会话模型</h2>
          <button type="button" className="icon-btn close-btn" onClick={onClose} disabled={saving}>
            ✕
          </button>
        </div>

        <div className="modal-section">
          <div className="mode-toggle-group">
            <button
              type="button"
              className={`mode-toggle-card ${mode === "default" ? "active" : ""}`}
              onClick={() => setMode("default")}
            >
              <div className="mode-toggle-title">跟随 Agent 默认</div>
              <div className="mode-toggle-desc">
                {agentDefaultModel ? (
                  <span className="mono">{agentDefaultModel.id}</span>
                ) : (
                  <span className="mode-toggle-hint">（Agent 暂未绑定默认模型）</span>
                )}
              </div>
            </button>

            <button
              type="button"
              className={`mode-toggle-card ${mode === "custom" ? "active" : ""}`}
              onClick={() => setMode("custom")}
            >
              <div className="mode-toggle-title">自定义会话模型</div>
              <div className="mode-toggle-desc">
                仅对当前会话生效，后续可随时切换
              </div>
            </button>
          </div>
        </div>

        {mode === "custom" && (
          <div className="modal-section custom-model-form">
            <div className="form-row">
              <label className="label" htmlFor="model-provider-select">供应商</label>
              {providers.length > 0 ? (
                <ChoiceSelect
                  id="model-provider-select"
                  value={selectedProviderId}
                  choices={providers.map((p) => ({
                    value: p.id,
                    label: `${p.name || p.id}（${p.api === "anthropic-messages" ? "Anthropic" : "OpenAI"} 协议）`,
                  }))}
                  onChange={handleProviderChange}
                  menuInPortal
                />
              ) : (
                <div className="form-warning">
                  未检测到配置的供应商，请先在右上角「设置」中添加供应商 API Key。
                </div>
              )}
            </div>

            <div className="form-row">
              <label className="label" htmlFor="session-model-id">模型</label>
              <ModelPicker
                id="session-model-id"
                providers={providers}
                providerId={selectedProviderId}
                modelId={modelId}
                disabled={saving}
                placeholder="如 gpt-4o、deepseek-chat、anthropic/claude-sonnet-4.5"
                onModelChange={(nextId, model) => {
                  setModelId(nextId);
                  // 手填（model 为 undefined）时保留已有限额：不能因为改了个 ID 就静默把
                  // 目录/用户先前的 maxTokens、contextWindow 复位成默认值。
                  setMaxTokens(resolveMaxTokens(model, maxTokens));
                  setContextWindow(resolveContextWindow(model, contextWindow));
                }}
              />
            </div>

            {entry && entry.models.length > 0 ? (
              <div className="form-row">
                <div className="hint">
                  {`模型列表来自 models.dev：${entry.name} 收录 ${entry.models.length} 个可工具调用的模型（来源 ${catalog ? catalogSourceLabel(catalog) : "目录"}）；目录外的模型直接输入模型 ID 回车即可。`}
                </div>
                {pickedModel && (
                  <div className="hint mono">
                    已选 {pickedModel.name} · 上下文 {formatTokens(pickedModel.context)} · 最大输出{" "}
                    {formatTokens(pickedModel.output)}
                  </div>
                )}
              </div>
            ) : (
              providers.length > 0 && (
                <div className="form-row">
                  <div className="hint">
                    该供应商不在模型目录里（自定义端点）：直接输入模型 ID 即可。
                  </div>
                </div>
              )
            )}
          </div>
        )}

        {modalError && <div className="form-error-banner">{modalError}</div>}

        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onClose} disabled={saving}>
            取消
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={handleSave}
            disabled={saving || (mode === "custom" && (!modelId.trim() || !selectedProviderId))}
          >
            {saving ? "切换中…" : "确认切换"}
          </button>
        </div>
      </div>
    </div>
  );
}
