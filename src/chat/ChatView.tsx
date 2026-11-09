import type { ReactNode } from "react";
import {
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { LayoutContext } from "../app/layout-context";
import { findSearchHits } from "./find";
import { useDialogFocus } from "../components/use-dialog-focus";
import {
  formatRuntimeError,
  groupChatBlocks,
  isAwaitingResponse,
  type Chip,
} from "../chat-runtime";
import ChatTimeline from "./ChatTimeline";
import Inspector from "./Inspector";
import { INSPECTOR_TABS, type InspectorTab } from "./inspector-tabs";
import { useI18n } from "../i18n";
import { IconClose } from "../icons";
import type { ChatInspectorContext } from "./inspector-context";
import ModelSelectModal from "./ModelSelectModal";
import AgentAvatar from "../components/AgentAvatar";
import ChatToolbar from "./ChatToolbar";
import ChatComposer from "./ChatComposer";
import { useResizableWidth } from "../resizable";
import { readStored, writeStored } from "../storage";
import type { SessionRuntime } from "../session-runtime";
import type { SessionController } from "../session-controller";
import type {
  AgentDefinition,
  ModelConfig,
  ProviderConfig,
  SessionInfoView,
} from "../types";

interface ChatViewProps {
  title?: string;
  agent: AgentDefinition;
  providers: ProviderConfig[];
  /** 正在查看的会话 id；null = 新会话（还没有文件，首次发送时创建）。 */
  sessionId: string | null;
  sessions: SessionRuntime;
  blockedSessionIds: string[];
  onBack?: () => void;
  onOpenFile?: (path: string) => void;
  onError: (msg: string) => void;
  onRunningChange: () => void;
  /** 用户主动停止成功后回报停止的会话 id，用于把侧栏状态行标成中断。 */
  onStopRequested?: (sessionId: string) => void;
  /** 新会话：传 null 表示无会话；分叉/重建后传新的会话信息以同步侧栏高亮。 */
  onSessionReset: (info?: SessionInfoView | null) => void;
  /** 保存当前控制器身份，以便后台新会话取得 id 时更新导航记忆。 */
  onControllerReady?: (controller: SessionController) => void;
  initialController?: SessionController;
  /** 设置工作台复用同一套对话状态机，但不暴露正式会话专属操作。 */
  mode?: "session" | "test";
  /** 临时测试的显式清空入口；返回后父组件用新 sessionId 重挂本视图。 */
  onClearTest?: () => Promise<void>;
  /** 设置工作台用自定义右栏；普通会话省略后继续渲染既有 Inspector。 */
  renderInspector?: (context: ChatInspectorContext) => ReactNode;
  /** 设置工作台在固定工具/thinking 标签时切换到测试页。 */
  onInspectDetail?: () => void;
}

export default function ChatView({
  title,
  agent,
  providers,
  sessionId: initialSessionId,
  sessions,
  blockedSessionIds,
  onBack,
  onOpenFile,
  onError,
  onRunningChange,
  onStopRequested,
  onSessionReset,
  onControllerReady,
  initialController,
  mode = "session",
  onClearTest,
  renderInspector,
  onInspectDetail,
}: ChatViewProps) {
  const { t } = useI18n();
  const displayTitle = title ?? t("chat.newSessionTitle");
  const isTest = mode === "test";
  const controller = useMemo(
    () => initialController ?? sessions.get(agent, initialSessionId, isTest),
    [sessions, agent.name, initialSessionId, isTest, initialController],
  );
  const layout = useContext(LayoutContext);
  const draft = useMemo(
    () =>
      sessions.composerDrafts.get(
        agent.name,
        controller.getSnapshot().sessionId,
        isTest,
      ),
    [sessions, controller, agent.name, isTest],
  );
  const snapshot = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
  );
  const { entries, stats, running } = snapshot.chat;
  const ready = snapshot.load === "ready";
  const stopping = snapshot.operation === "stopping";
  const sessionId = snapshot.sessionId;
  const sessionModel = snapshot.model;
  const isCustomModel = snapshot.isCustomModel;
  const connecting = snapshot.operation !== "uncertain" && ready && !stopping && isAwaitingResponse(snapshot.chat);
  const [atTail, setAtTail] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [pinnedFindKey, setPinnedFindKey] = useState<string | null>(null);
  const [previewFindKey, setPreviewFindKey] = useState<string | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [clearingTest, setClearingTest] = useState(false);
  const [pinnedChip, setPinnedChip] = useState<string | null>(null);
  const [previewChip, setPreviewChip] = useState<string | null>(null);
  const [modelModalOpen, setModelModalOpen] = useState(false);
  const [activeInspectorTab, setActiveInspectorTab] =
    useState<InspectorTab>(() => {
      if (isTest) return "state";
      const stored = readStored("pipi-inspector-tab", "state");
      return INSPECTOR_TABS.some((item) => item.id === stored)
        ? (stored as InspectorTab)
        : "state";
    });
  // 「调用详情」是临时的：没有 pinned chip 时不能作为恢复目标。
  const previousTabRef = useRef<InspectorTab>(
    activeInspectorTab === "detail" ? "state" : activeInspectorTab,
  );

  const [inspectorOpen, setInspectorOpen] = useState(() => {
    if (isTest) {
      return typeof window === "undefined" || window.innerWidth > 1100;
    }
    // 窄屏用抽屉，不自动弹出；宽屏沿用用户上次的选择。
    if (typeof window !== "undefined" && window.innerWidth <= 1100) return false;
    return readStored("pipi-inspector-open", "0") === "1";
  });

  // 切换会话会重挂 ChatView（App 的 chatKey）；右栏开关与 tab 必须跨重挂保留，
  // 否则从仓库树里点一个分支，右栏就关掉了。
  useEffect(() => {
    if (isTest) return;
    // 窄屏是抽屉：不覆盖宽屏下的开关偏好。
    if (typeof window !== "undefined" && window.innerWidth <= 1100) return;
    writeStored("pipi-inspector-open", inspectorOpen ? "1" : "0");
  }, [inspectorOpen, isTest]);
  useEffect(() => {
    // 详情页由 pinned chip 驱动，跨会话没有意义，不持久化。
    if (!isTest && activeInspectorTab !== "detail") {
      writeStored("pipi-inspector-tab", activeInspectorTab);
    }
  }, [activeInspectorTab, isTest]);
  // 检查器宽度可拖拽调节（窄屏抽屉模式由 styles/components/panels.css 接管）。
  const inspectorResize = useResizableWidth({
    storageKey: isTest ? "agent-settings-width" : "inspector-width",
    initial: isTest ? 400 : 320,
    min: isTest ? 340 : 260,
    max: 620,
    edge: "left",
  });
  const [narrow, setNarrow] = useState(
    () => typeof window !== "undefined" && window.innerWidth <= 1100,
  );
  const listRef = useRef<HTMLDivElement>(null);
  const chatRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLDivElement>(null);
  const inspectorRef = useRef<HTMLElement>(null);
  useDialogFocus(
    inspectorRef,
    () => setInspectorOpen(false),
    narrow && inspectorOpen,
  );
  const mountedRef = useRef(true);

  // 渲染分组：连续的工具调用 / thinking 折叠为一行标签
  const blocks = useMemo(() => groupChatBlocks(entries), [entries]);
  const searchHits = useMemo(
    () => findSearchHits(blocks, searchQuery),
    [blocks, searchQuery],
  );
  const findHits = useMemo(() => searchHits.map((h) => h.key), [searchHits]);
  const activeFindKey = previewFindKey ?? pinnedFindKey ?? null;

  // 搜索词变更导致原选中项不在新结果中时清掉引用
  useEffect(() => {
    if (pinnedFindKey && !findHits.includes(pinnedFindKey)) {
      setPinnedFindKey(null);
    }
    if (previewFindKey && !findHits.includes(previewFindKey)) {
      setPreviewFindKey(null);
    }
  }, [findHits, pinnedFindKey, previewFindKey]);
  const chipById = useMemo(() => {
    const map = new Map<string, Chip>();
    for (const block of blocks) {
      if (block.kind !== "chips") continue;
      for (const chip of block.chips) map.set(chip.id, chip);
    }
    return map;
  }, [blocks]);
  const activeChipId = previewChip ?? pinnedChip;
  const activeChip = activeChipId ? (chipById.get(activeChipId) ?? null) : null;
  // 条目被整体替换（切会话 / 水合）后固定项可能已不存在：清掉悬空引用
  useEffect(() => {
    if (pinnedChip && !chipById.has(pinnedChip)) setPinnedChip(null);
    if (previewChip && !chipById.has(previewChip)) setPreviewChip(null);
  }, [chipById, pinnedChip, previewChip]);
  useEffect(() => {
    if (activeChipId) {
      if (activeInspectorTab !== "detail") {
        previousTabRef.current = activeInspectorTab;
        setActiveInspectorTab("detail");
      }
    } else if (activeInspectorTab === "detail") {
      setActiveInspectorTab(previousTabRef.current);
    }
  }, [activeChipId]);
  const followTailRef = useRef(true);
  // 首次贴底前忽略顶部 scroll 事件：它可能来自挂载/测量的程序化滚动，
  // 不能把「跟随底部」误判成用户主动阅读历史。
  const pinnedOnceRef = useRef(false);
  const composingRef = useRef(false);
  const lastErrorRef = useRef(0);
  const lastChangeRef = useRef(snapshot.change?.revision ?? 0);

  useEffect(() => {
    pinnedOnceRef.current = false;
    followTailRef.current = true;
    setAtTail(true);
  }, [sessionId]);

  // 输入区覆盖在消息上方；按实际高度预留滚动空间，包含多行草稿与安全提示。
  useLayoutEffect(() => {
    const chat = chatRef.current;
    const composer = composerRef.current;
    const list = listRef.current;
    if (!chat || !composer || !list) return;
    const update = () => {
      chat.style.setProperty("--composer-height", `${composer.offsetHeight}px`);
      if (followTailRef.current) {
        list.scrollTo({ top: list.scrollHeight, behavior: "auto" });
      }
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(composer);
    observer.observe(list);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    controller.updateAgent(agent);
  }, [controller, agent]);
  useEffect(() => {
    onControllerReady?.(controller);
  }, [controller, onControllerReady]);
  useEffect(() => {
    let active = true;
    mountedRef.current = true;
    if (initialController) sessions.retainDraft(initialController);
    void sessions
      .start()
      .then(() => {
        // 新会话可能正在后台获取身份；重进视图时不能把它重置为空会话。
        const current = controller.getSnapshot();
        if (current.sessionId === null && current.load === "ready") return;
        return controller.hydrate();
      })
      .catch((error) => {
        if (active) onError(formatRuntimeError(error));
      });
    return () => {
      active = false;
      mountedRef.current = false;
    };
  }, [controller, sessions, onError, initialController]);
  useEffect(() => {
    if (snapshot.error && snapshot.error.revision > lastErrorRef.current) {
      lastErrorRef.current = snapshot.error.revision;
      onError(snapshot.error.message);
    }
  }, [snapshot.error, onError]);
  useEffect(() => {
    if (snapshot.change && snapshot.change.revision > lastChangeRef.current) {
      lastChangeRef.current = snapshot.change.revision;
      onSessionReset(snapshot.change.info);
    }
  }, [snapshot.change, onSessionReset]);
  // 检查器在窄屏是抽屉：跨过断点时对齐默认值（宽屏展开 / 窄屏收起），
  // 不覆盖用户在同一布局下的手动切换。
  useEffect(() => {
    const onResize = () => setNarrow(window.innerWidth <= 1100);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    setInspectorOpen(isTest && !narrow);
  }, [narrow, isTest]);

  useEffect(() => {
    if (layout.navigationOpen && narrow) setInspectorOpen(false);
  }, [layout.navigationOpen, narrow]);

  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (
        event.isComposing ||
        composingRef.current ||
        document.querySelector('[aria-modal="true"]')
      )
        return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        layout.closeNavigation();
        setActiveInspectorTab("search");
        setInspectorOpen(true);
        searchInputRef.current?.focus();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [layout]);

  // 搜索命中由 ChatTimeline 按块索引滚动（虚拟列表里目标可能还没挂载）；
  // 这里只负责离开「跟随底部」状态，避免滚动目标与流式贴底互相拉扯。
  useEffect(() => {
    if (!inspectorOpen || activeInspectorTab !== "search" || !activeFindKey)
      return;
    followTailRef.current = false;
    setAtTail(false);
  }, [inspectorOpen, activeInspectorTab, activeFindKey]);

  useEffect(() => {
    if (!narrow || !inspectorOpen) return;
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setInspectorOpen(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [inspectorOpen, narrow]);

  // 流式贴底由 ChatTimeline 在虚拟行测量完成后处理（见其 pinnedToBottom effect）。
  useEffect(() => {
    onRunningChange();
  }, [onRunningChange, running]);

  const handleScroll = () => {
    const list = listRef.current;
    if (!list) return;
    // 空会话（欢迎页/水合前）的 scrollHeight == clientHeight，不能据此把
    // 「首次贴底」标记成已完成；等条目到达后再让 ChatTimeline 贴底。
    if (entries.length === 0) return;
    const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight <= 48;
    if (!pinnedOnceRef.current && !atBottom) return;
    if (atBottom) pinnedOnceRef.current = true;
    followTailRef.current = atBottom;
    setAtTail(atBottom);
  };
  const scrollToLatest = () => {
    followTailRef.current = true;
    setAtTail(true);
    const list = listRef.current;
    if (list) list.scrollTo({ top: list.scrollHeight, behavior: "auto" });
  };
  const clearPreviewDetail = () => setPreviewChip(null);
  const togglePinnedDetail = (id: string) => {
    setPinnedChip((current) => (current === id ? null : id));
    setPreviewChip(null);
    setActiveInspectorTab("detail");
    onInspectDetail?.();
    layout.closeNavigation();
    setInspectorOpen(true);
  };
  const stop = () => {
    void controller.stop()
      .then(() => {
        // stop() 只有在核心确认已停（或超时后仍不确定）才返回；仍在跑就不改状态。
        const stopped = controller.getSnapshot();
        if (!stopped.chat.running && stopped.sessionId) onStopRequested?.(stopped.sessionId);
      })
      .catch(() => {});
  };
  const compactNow = () => {
    void controller
      .compact()
      .catch((error) => onError(formatRuntimeError(error)));
  };
  const agentCompactNow = () => {
    void controller
      .compact("edit")
      .catch((error) => onError(formatRuntimeError(error)));
  };
  const forkSession = () => {
    void controller.fork().catch((error) => onError(formatRuntimeError(error)));
  };
  const handleModelChange = async (target: {
    isCustom: boolean;
    model: ModelConfig | null;
  }) => {
    await controller.setModel(target.isCustom, target.model);
    setModelModalOpen(false);
  };
  const clearTest = async () => {
    if (!isTest || !onClearTest || running || clearingTest) return;
    setClearingTest(true);
    try {
      await onClearTest();
    } catch (error) {
      onError(formatRuntimeError(error));
    } finally {
      if (mountedRef.current) setClearingTest(false);
    }
  };

  const inspectorContext: ChatInspectorContext = {
    agent,
    sessionId,
    entries,
    stats,
    running,
    ready,
    sessionModel,
    isCustomModel,
    blockedCount: blockedSessionIds.length,
    chipDetail: activeChip,
    chipDetailMode: previewChip ? "preview" : "pinned",
    onUnpinChip: () => setPinnedChip(null),
    runtime: sessions,
  };

  return (
    <div
      className={`chat-shell${inspectorOpen ? " inspector-open" : ""}${isTest ? " test-workbench" : ""}`}
      style={
        {
          "--inspector-width": `${inspectorResize.width}px`,
        } as React.CSSProperties
      }
    >
      <div className="content-pane chat-main">
      <ChatToolbar
        agentName={agent.name}
        title={displayTitle}
        running={running}
        temporary={isTest}
        ready={ready}
        clearing={clearingTest}
        inspectorOpen={inspectorOpen}
        hasDetail={Boolean(activeChip)}
        onInspectorToggle={() => setInspectorOpen((open) => !open)}
        onBack={onBack}
        onClear={onClearTest ? () => void clearTest() : undefined}
      />

      <div className="chat-body">
        <div
          className={`chat${!isTest && entries.length === 0 ? " empty-chat" : ""}`}
          ref={chatRef}
        >
          <div
            className="log"
            ref={listRef}
            onScroll={handleScroll}
            aria-live={running ? "polite" : undefined}
          >
            {entries.length === 0 && (
              <div className="chat-welcome">
                {isTest ? (
                  <>
                    {t("chat.testWelcomePrefix")}{" "}
                    <span className="mono">{agent.name}</span>
                    {t("chat.testWelcomeSuffix")}
                  </>
                ) : (
                  <>
                    <AgentAvatar
                      name={agent.name}
                      subagent={agent.subagent}
                      className="welcome-avatar"
                    />
                    <span className="caption welcome-kicker">{agent.name}</span>
                    <h2>
                      {t("chat.welcomeTitle")}
                    </h2>
                    <p>
                      {t("chat.welcomeBody")}
                    </p>
                  </>
                )}
              </div>
            )}
            <ChatTimeline
              agent={agent}
              connecting={connecting}
              blocks={blocks}
              scrollRef={listRef}
              pinnedToBottom={atTail}
              activeChipId={activeChipId}
              pinnedChip={pinnedChip}
              findHits={
                inspectorOpen && activeInspectorTab === "search" ? findHits : []
              }
              activeFindKey={
                inspectorOpen && activeInspectorTab === "search"
                  ? activeFindKey
                  : null
              }
              previewFindKey={
                inspectorOpen && activeInspectorTab === "search"
                  ? previewFindKey
                  : null
              }
              pinnedFindKey={
                inspectorOpen && activeInspectorTab === "search"
                  ? pinnedFindKey
                  : null
              }
              previewDetail={setPreviewChip}
              clearPreviewDetail={clearPreviewDetail}
              togglePinnedDetail={togglePinnedDetail}
              onFork={!isTest ? forkSession : undefined}
              forkDisabled={!ready || running || !sessionId}
            />
          </div>

          {snapshot.load === "failed" && (
            <div className="chat-load-failed" role="alert">
              <span>{t("chat.loadFailed")}</span>
              <button type="button" className="btn" onClick={() => void controller.hydrate()}>
                {t("common.retry")}
              </button>
            </div>
          )}
          {snapshot.operation === "uncertain" && (
            <div className="chat-send-uncertain" role="status">
              {t("chat.sendUncertain")}
            </div>
          )}
          <ChatComposer
            containerRef={composerRef}
            draft={draft}
            ready={ready}
            running={running}
            stopping={stopping}
            uncertain={snapshot.operation === "uncertain"}
            temporary={isTest}
            atTail={atTail}
            model={sessionModel}
            customModel={isCustomModel}
            composingRef={composingRef}
            onSend={(text, images) => controller.send(text, images)}
            onSteer={(text, images) => controller.steer(text, images)}
            onQueue={(text, delayMs, images) => controller.enqueue(text, delayMs, images)}
            queue={snapshot.queue}
            onCancelQueued={(id) => controller.cancelQueued(id)}
            onPauseQueue={(paused) => controller.pauseQueue(paused)}
            onBeforeSend={() => {
              followTailRef.current = true;
              setAtTail(true);
            }}
            onStop={stop}
            onOpenModel={() => setModelModalOpen(true)}
            onReasoningChange={(effort) => controller.setReasoningEffort(effort)}
            onScrollToLatest={scrollToLatest}
            onError={onError}
          />
        </div>
      </div>
      </div>

      {inspectorOpen && (
          <div
            className="resize-handle inspector-resize"
            title={t("inspector.resizeHandleTitle")}
            aria-label={t("inspector.resizeHandle")}
            {...inspectorResize.handleProps}
          />
        )}

        {renderInspector ? (
          <aside
            ref={inspectorRef}
            inert={!inspectorOpen}
            aria-hidden={!inspectorOpen || undefined}
            className="inspector agent-settings-inspector"
            id="session-inspector"
            aria-label={t("inspector.agentSettingsAndTest")}
          >
            {narrow && (
              <button
                type="button"
                className="icon-btn agent-settings-close"
                onClick={() => setInspectorOpen(false)}
                aria-label={t("inspector.closeAgentSettings")}
                title={t("inspector.closeAgentSettings")}
              >
                <IconClose />
              </button>
            )}
            {renderInspector(inspectorContext)}
          </aside>
        ) : (
          <Inspector
            context={inspectorContext}
            onCompact={compactNow}
            onAgentCompact={agentCompactNow}
            onOpenFile={onOpenFile}
            hidden={!inspectorOpen}
            rootRef={inspectorRef}
            activeTab={activeInspectorTab}
            onTabChange={setActiveInspectorTab}
            tabStyle="icon"
            onClose={() => setInspectorOpen(false)}
            searchQuery={searchQuery}
            onSearchQueryChange={setSearchQuery}
            searchHits={searchHits}
            pinnedFindKey={pinnedFindKey}
            previewFindKey={previewFindKey}
            onPreviewFind={(key) => {
              followTailRef.current = false;
              setAtTail(false);
              setPreviewFindKey(key);
            }}
            onPinFind={(key) => {
              followTailRef.current = false;
              setAtTail(false);
              setPinnedFindKey(key);
              setPreviewFindKey(null);
            }}
            searchInputRef={searchInputRef}
          />
        )}

      {narrow && inspectorOpen && (
        <button
          type="button"
          className="inspector-backdrop"
          aria-label={t("inspector.close")}
          onClick={() => setInspectorOpen(false)}
        />
      )}

      {!isTest && modelModalOpen && (
        <ModelSelectModal
          isOpen={modelModalOpen}
          onClose={() => setModelModalOpen(false)}
          onConfirm={handleModelChange}
          currentModel={sessionModel}
          isCustomModel={isCustomModel}
          agentDefaultModel={agent.provider ?? null}
          providers={providers}
        />
      )}
    </div>
  );
}
