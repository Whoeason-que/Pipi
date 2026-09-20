import { useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { Markdown } from "./Markdown";
import { invoke, listen } from "./platform";
import { useResizableWidth } from "./resizable";
import { IconBack, IconCheck, IconCopy, IconFork, IconGrid, IconPlus, IconSend, IconStop } from "./icons";
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
  groupChatBlocks,
  INITIAL_CHAT_STATE,
  normalizeAgentEvent,
  normalizeStatsPayload,
  type AgentEvent,
  type AgentEventPayload,
  type ApprovalDecisionValue,
  type ApprovalRequestPayload,
  type ChatEntry,
  type Chip,
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
  /** 待处理的 bash 命令审批请求（同时至多一个：bash 工具强制顺序执行）。 */
  const [pendingApproval, setPendingApproval] = useState<ApprovalRequestPayload | null>(null);
  /** 右栏「调用详情」：悬浮标签 = 临时预览，点击 = 固定（再点取消固定）。 */
  const [pinnedChip, setPinnedChip] = useState<string | null>(null);
  const [previewChip, setPreviewChip] = useState<string | null>(null);
  const [sessionModel, setSessionModel] = useState<ModelConfig | null>(agent.provider ?? null);
  const [isCustomModel, setIsCustomModel] = useState(false);
  const [modelModalOpen, setModelModalOpen] = useState(false);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [inspectorOpen, setInspectorOpen] = useState(
    () => typeof window === "undefined" || window.innerWidth > 1100,
  );
  // 检查器宽度可拖拽调节（窄屏抽屉模式下由媒体查询接管，见 responsive.css）
  const inspectorResize = useResizableWidth({
    storageKey: "inspector-width",
    initial: 320,
    min: 260,
    max: 620,
    edge: "left",
  });
  const [narrow, setNarrow] = useState(
    () => typeof window !== "undefined" && window.innerWidth <= 1100,
  );
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const mountedRef = useRef(true);

  // 渲染分组：连续的工具调用 / thinking 折叠为一行标签
  const blocks = useMemo(() => groupChatBlocks(entries), [entries]);
  const chipById = useMemo(() => {
    const map = new Map<string, Chip>();
    for (const block of blocks) {
      if (block.kind !== "chips") continue;
      for (const chip of block.chips) map.set(chip.id, chip);
    }
    return map;
  }, [blocks]);
  const activeChipId = previewChip ?? pinnedChip;
  const activeChip = activeChipId ? chipById.get(activeChipId) ?? null : null;
  // 条目被整体替换（切会话 / 水合）后固定项可能已不存在：清掉悬空引用
  useEffect(() => {
    if (pinnedChip && !chipById.has(pinnedChip)) setPinnedChip(null);
    if (previewChip && !chipById.has(previewChip)) setPreviewChip(null);
  }, [chipById, pinnedChip, previewChip]);
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
      // 运行已结束：残留的审批请求在核心侧必然已 fail-closed，收起横幅
      setPendingApproval(null);
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
    const handleApprovalEvent = (event: { payload: ApprovalRequestPayload }) => {
      if (!mounted || sessionInfoFailedRef.current) return;
      const request = event.payload;
      if (request.agentName !== agent.name) return;
      const identity = sessionIdentityRef.current;
      // 请求属于当前打开的会话才弹窗；过期请求由核心超时兜底
      if (identity && request.sessionId !== identity.sessionId) return;
      setPendingApproval(request);
    };

    const initialize = async () => {
      const listenerResults = await Promise.allSettled([
        listen<AgentEventPayload>("agent-event", handleAgentEvent),
        listen<SessionStatsPayload>("session-stats", handleStatsEvent),
        listen<SessionErrorPayload>("session-error", handleErrorEvent),
        listen<ApprovalRequestPayload>("approval-request", handleApprovalEvent),
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
      setPendingApproval(null);
      setPinnedChip(null);
      setPreviewChip(null);
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
    if (!text || sendInFlightRef.current) return;

    // 运行中输入 = steering（插话）：注入当前运行的下一轮上下文。
    // 消息本体由核心注入时经事件回流渲染，这里不做乐观插入。
    if (running) {
      sendInFlightRef.current = true;
      setInput("");
      try {
        await invoke("steer", { message: text });
      } catch (error) {
        // 运行恰好结束等竞态：恢复输入，让用户重发
        setInput(text);
        onError(formatRuntimeError(error));
      } finally {
        sendInFlightRef.current = false;
      }
      return;
    }

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

  /** 回传审批请求的用户决定；决定后立即收起横幅（过期请求由核心 fail-closed）。 */
  const resolveApproval = async (decision: ApprovalDecisionValue) => {
    const request = pendingApproval;
    if (!request) return;
    setPendingApproval(null);
    try {
      await invoke("resolve_approval", { requestId: request.requestId, decision });
    } catch (error) {
      onError(formatRuntimeError(error));
    }
  };

  // 标签交互：悬浮临时预览、移开还原；点击固定/取消固定（窄屏同时展开抽屉）
  const previewDetail = (id: string) => setPreviewChip(id);
  const clearPreviewDetail = () => setPreviewChip(null);
  const togglePinnedDetail = (id: string) => {
    setPinnedChip((current) => (current === id ? null : id));
    setPreviewChip(null);
    if (narrow) setInspectorOpen(true);
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
    <div
      className={`chat-shell${inspectorOpen ? " inspector-open" : ""}`}
      style={{ "--inspector-width": `${inspectorResize.width}px` } as React.CSSProperties}
    >
      <div className="chat">
        <div className="screen-bar">
          <button type="button" className="icon-btn" title="返回" aria-label="返回" onClick={onBack}>
            <IconBack />
          </button>
          <span className="crumb" title={`~/.pipi/agents/${agent.name}/sessions/${sessionId ?? ""}`}>
            ~/.pipi/agents/<b>{agent.name}</b>/sessions/{sessionId ? <b>{sessionId}</b> : "…"}
          </span>
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
          {blocks.map((block) => {
            if (block.kind === "system") {
              const entry = block.entry;
              return (
                <div key={entry.key} className="row system-row">
                  <div className="row-inner">
                    <details className="compaction-fold" open={!entry.summary}>
                      <summary>{entry.text}</summary>
                      {entry.summary ? <Markdown text={entry.summary} /> : null}
                    </details>
                  </div>
                </div>
              );
            }
            if (block.kind === "chips") {
              return (
                <ChipRow
                  key={block.key}
                  chips={block.chips}
                  activeId={activeChipId}
                  pinnedId={pinnedChip}
                  onPreview={previewDetail}
                  onPreviewEnd={clearPreviewDetail}
                  onTogglePin={togglePinnedDetail}
                />
              );
            }
            const entry = block.entry;
            if (block.kind === "user") {
              return (
                <div key={entry.key} className="row user-row">
                  <div className="row-inner">
                    <div className="user-bubble">
                      <div className="user-text">{entry.text}</div>
                    </div>
                    <div className="row-meta">
                      {entry.timestamp ? (
                        <span className="time">{formatClock(entry.timestamp)}</span>
                      ) : null}
                      <CopyButton text={entry.text} />
                    </div>
                  </div>
                </div>
              );
            }
            // 正文块：assistant 文本（thinking 与工具调用已由标签行承载）
            const footer = !entry.streaming && !entry.status
              ? assistantFooter({
                  role: "assistant",
                  content: entry.text,
                  usage: entry.usage,
                  durationMs: entry.durationMs,
                })
              : null;
            return (
              <div key={entry.key} className="row">
                <div className="row-inner">
                  <div className="content">
                    <Markdown text={entry.text} />
                    {entry.streaming && <span className="cursor" aria-hidden="true" />}
                  </div>
                  {!entry.streaming && (
                    // 统计信息与时间、按钮同一行；时间与按钮靠右（hover 时淡入）
                    <div className="row-meta">
                      {footer && <span className="hint meta-info">{footer}</span>}
                      <span className="spacer" />
                      {entry.timestamp ? (
                        <span className="time">{formatClock(entry.timestamp)}</span>
                      ) : null}
                      <CopyButton text={entry.text} />
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>

        <div className="composer">
          {pendingApproval && (
            <div className="approval-bar" role="alertdialog" aria-label="命令执行审批">
              <div className="approval-text">
                <span className="approval-title">Agent 请求执行白名单外的命令</span>
                <code>{pendingApproval.command}</code>
              </div>
              <div className="approval-actions">
                <button type="button" className="btn deny" onClick={() => void resolveApproval("deny")}>
                  拒绝
                </button>
                <button type="button" className="btn allow" onClick={() => void resolveApproval("allow")}>
                  允许一次
                </button>
                <button type="button" className="btn allow" onClick={() => void resolveApproval("always")} title="执行并把命令加入 Agent 白名单（agent.json）">
                  总是允许
                </button>
              </div>
            </div>
          )}
          <div className="composer-box">
            <textarea
              ref={inputRef}
              value={input}
              onChange={(event) => setInput(event.target.value)}
              placeholder={
                ready
                  ? running
                    ? "Agent 正在运行…输入内容将作为插话（steering）注入"
                    : "输入消息，Enter 发送（Shift+Enter 换行）"
                  : "正在连接 Agent…"
              }
              disabled={!ready}
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
              {running && (
                <button
                  type="button"
                  className="btn stop"
                  onClick={stop}
                  disabled={stopping}
                  title={stopping ? "停止中…" : "停止"}
                  aria-label="停止运行"
                >
                  <IconStop />
                </button>
              )}
              <button
                type="button"
                className="btn send"
                disabled={!ready || !sessionInfoResolvedRef.current || !input.trim()}
                onClick={() => void send()}
                title={running ? "插话（steering）" : "发送"}
                aria-label={running ? "插话" : "发送消息"}
              >
                <IconSend />
              </button>
            </div>
          </div>
        </div>
      </div>

      {inspectorOpen && (
        <div
          className="resize-handle inspector-resize"
          title="拖动调整检查器宽度（←/→ 微调）"
          aria-label="调整检查器宽度"
          {...inspectorResize.handleProps}
        />
      )}

      <Inspector
        agent={agent}
        sessionId={sessionId}
        entries={entries}
        stats={stats}
        running={running}
        sessionModel={sessionModel}
        isCustomModel={isCustomModel}
        blockedCount={blockedSessionIds.length}
        chipDetail={activeChip}
        chipDetailMode={previewChip ? "preview" : "pinned"}
        onUnpinChip={() => setPinnedChip(null)}
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

/** 消息操作：复制正文（hover 出现的操作行里，成功后短暂显示对勾） */
function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={`icon-btn copy-btn${copied ? " copied" : ""}`}
      title={copied ? "已复制" : "复制"}
      aria-label={copied ? "已复制" : "复制消息"}
      disabled={!text}
      onClick={() => {
        navigator.clipboard
          ?.writeText(text)
          .then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          })
          .catch(() => {
            // 剪贴板不可用时静默失败，不打断阅读
          });
      }}
    >
      {copied ? <IconCheck /> : <IconCopy />}
    </button>
  );
}

function formatTokens(value: number | undefined): string {
  if (value == null) return "—";
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

/**
 * 正文里的标签行：连续的工具调用 / thinking 压成一行（保持顺序、不合并）。
 * 悬浮 = 右栏临时预览，点击 = 固定（再点取消）；状态由标签自身承载：
 * 运行中脉冲、失败红色、成功低调。
 */
function ChipRow({
  chips,
  activeId,
  pinnedId,
  onPreview,
  onPreviewEnd,
  onTogglePin,
}: {
  chips: Chip[];
  activeId: string | null;
  pinnedId: string | null;
  onPreview: (id: string) => void;
  onPreviewEnd: () => void;
  onTogglePin: (id: string) => void;
}) {
  return (
    <div className="row chip-line">
      <div className="row-inner">
        <div className="chip-row">
          {chips.map((chip) => {
            const classes = ["chip", `chip-${chip.status}`];
            if (chip.id === activeId) classes.push("active");
            if (chip.id === pinnedId) classes.push("pinned");
            return (
              <button
                key={chip.id}
                type="button"
                className={classes.join(" ")}
                onMouseEnter={() => onPreview(chip.id)}
                onMouseLeave={onPreviewEnd}
                onFocus={() => onPreview(chip.id)}
                onBlur={onPreviewEnd}
                onClick={() => onTogglePin(chip.id)}
                title={`${chip.name} · ${
                  chip.status === "running" ? "运行中" : chip.status === "error" ? "失败" : "已完成"
                }（悬浮预览 · 点击固定到右栏）`}
                aria-expanded={chip.id === pinnedId}
              >
                {chip.status === "running" && <span className="chip-dot" aria-hidden="true" />}
                {chip.status === "error" && <span className="chip-x" aria-hidden="true">✕</span>}
                <span className="chip-name">{chip.name}</span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
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
    <div className={`tool${running ? " tool-running" : failed ? " tool-error" : ""}`}>
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
              <pre className="code">{toolOutputBody(entry) || (running ? "执行中…" : "（无输出）")}</pre>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/** 去除 runtime 前缀（⚙ 工具名 / ✕）后的工具输出正文。 */
function toolOutputBody(entry: ChatEntry): string {
  const text = entry.text ?? "";
  const okPrefix = `⚙ ${entry.toolName ?? "tool"}`;
  if (text.startsWith(okPrefix)) {
    return text.slice(okPrefix.length).replace(/^\n/, "");
  }
  if (text.startsWith("✕ ")) return text.slice(2);
  return text;
}

/**
 * 右栏「调用详情」面板：承载被选中标签的完整内容。
 * - 工具标签：该次调用的可折叠卡（参数 / 输出 / diff，上下排列）
 * - thinking 标签：该段思考正文
 * - 固定（pinned）时显示取消固定按钮；悬浮预览时标注「预览」
 */
function ChipDetailPane({
  chip,
  entries,
  mode,
  canUnpin,
  onUnpin,
}: {
  chip: Chip;
  entries: ChatEntry[];
  mode: "preview" | "pinned";
  canUnpin: boolean;
  onUnpin: () => void;
}) {
  const entry = entries.find((candidate) => candidate.key === chip.entryKey);
  return (
    <div className="detail-pane">
      <div className="detail-head">
        <span className={`detail-name chip-${chip.status}`}>{chip.name}</span>
        <span className="detail-mode">{mode === "preview" ? "预览" : "已固定"}</span>
        <span className="spacer" />
        {canUnpin && (
          <button
            type="button"
            className="icon-btn detail-unpin"
            onClick={onUnpin}
            title="取消固定"
            aria-label="取消固定"
          >
            ×
          </button>
        )}
      </div>
      <div className="detail-body">
        {chip.kind === "thinking" ? (
          <pre className="thinking-body">{chip.thinking ?? entry?.thinking ?? ""}</pre>
        ) : entry ? (
          <ToolResultCard entry={entry} />
        ) : (
          <div className="detail-empty">该条目的记录已不在当前会话中。</div>
        )}
      </div>
    </div>
  );
}

// ============ 右侧检查器 ============

type InspectorTab = "detail" | "stats" | "state" | "files";

interface InspectorProps {
  agent: AgentDefinition;
  sessionId: string | null;
  entries: ChatEntry[];
  stats: SessionStatsView | null;
  running: boolean;
  sessionModel: ModelConfig | null;
  isCustomModel: boolean;
  blockedCount: number;
  /** 被选中的标签组（悬浮预览或固定）；null 表示没有选中项。 */
  chipDetail: Chip | null;
  chipDetailMode: "preview" | "pinned";
  onUnpinChip: () => void;
}

const INSPECTOR_TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: "detail", label: "调用详情" },
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
  chipDetail,
  chipDetailMode,
  onUnpinChip,
}: InspectorProps) {
  const [tab, setTab] = useState<InspectorTab>("stats");
  // 选中标签时自动切到「调用详情」，取消选中后回到之前的 tab
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const previousTabRef = useRef<InspectorTab>("stats");
  const detailKey = chipDetail ? chipDetail.id : null;
  useEffect(() => {
    if (detailKey) {
      if (tabRef.current !== "detail") {
        previousTabRef.current = tabRef.current;
        setTab("detail");
      }
    } else if (tabRef.current === "detail") {
      setTab(previousTabRef.current);
    }
  }, [detailKey]);

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
          className={`ipanel${tab === "detail" ? " active" : ""}`}
          id="ipanel-detail"
          role="tabpanel"
          aria-labelledby="itab-detail"
        >
          {chipDetail ? (
            <ChipDetailPane
              chip={chipDetail}
              entries={entries}
              mode={chipDetailMode}
              canUnpin={chipDetailMode === "pinned"}
              onUnpin={onUnpinChip}
            />
          ) : (
            <div className="detail-empty">
              悬浮对话里的工具 / thinking 标签临时预览，点击固定到右栏。
            </div>
          )}
        </div>
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
                      <span className="mono" style={{ color: "var(--fg)" }}>
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
