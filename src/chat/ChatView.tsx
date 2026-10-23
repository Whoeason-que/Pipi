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
  type ApprovalDecisionValue,
  type Chip,
} from "../chat-runtime";
import ChatTimeline from "./ChatTimeline";
import Inspector from "./Inspector";
import type { InspectorTab } from "./inspector-tabs";
import { useI18n } from "../i18n";
import { IconClose } from "../icons";
import type { ChatInspectorContext } from "./inspector-context";
import ModelSelectModal from "./ModelSelectModal";
import AgentAvatar from "../components/AgentAvatar";
import ChatToolbar from "./ChatToolbar";
import ChatComposer from "./ChatComposer";
import { useResizableWidth } from "../resizable";
import type { SessionRuntime } from "../session-runtime";
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
  onNewSession?: (previousSessionId?: string) => Promise<boolean>;
  onRunningChange: () => void;
  /** 新会话：传 null 表示无会话；分叉/重建后传新的会话信息以同步侧栏高亮。 */
  onSessionReset: (info?: SessionInfoView | null) => void;
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
  title = "新会话",
  agent,
  providers,
  sessionId: initialSessionId,
  sessions,
  blockedSessionIds,
  onBack,
  onOpenFile,
  onError,
  onNewSession,
  onRunningChange,
  onSessionReset,
  mode = "session",
  onClearTest,
  renderInspector,
  onInspectDetail,
}: ChatViewProps) {
  const { locale, t } = useI18n();
  const isTest = mode === "test";
  const controller = useMemo(
    () => sessions.get(agent, initialSessionId, isTest),
    [sessions, agent.name, initialSessionId, isTest],
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
  const pendingApproval = snapshot.approval;
  const connecting = snapshot.operation !== "uncertain" && ready && !stopping && !pendingApproval && isAwaitingResponse(snapshot.chat);
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
    useState<InspectorTab>("stats");
  const previousTabRef = useRef<InspectorTab>("stats");

  const [inspectorOpen, setInspectorOpen] = useState(
    () => isTest && (typeof window === "undefined" || window.innerWidth > 1100),
  );
  // 检查器宽度可拖拽调节（窄屏抽屉模式下由媒体查询接管，见 responsive.css）
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
  const composingRef = useRef(false);
  const lastErrorRef = useRef(0);
  const lastChangeRef = useRef(snapshot.change?.revision ?? 0);

  // 输入区覆盖在消息上方；按实际高度预留滚动空间，包含多行草稿、审批与安全提示。
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
    let active = true;
    mountedRef.current = true;
    void sessions
      .start()
      .then(() => controller.hydrate())
      .catch((error) => {
        if (active) onError(formatRuntimeError(error));
      });
    return () => {
      active = false;
      mountedRef.current = false;
    };
  }, [controller, sessions, onError]);
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

  useEffect(() => {
    if (!inspectorOpen || activeInspectorTab !== "search" || !activeFindKey)
      return;
    followTailRef.current = false;
    setAtTail(false);
    const target = listRef.current?.querySelector(
      `[data-entry-key="${CSS.escape(activeFindKey)}"]`,
    );
    target?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [inspectorOpen, activeInspectorTab, activeFindKey]);

  useEffect(() => {
    if (!narrow || !inspectorOpen) return;
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setInspectorOpen(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [inspectorOpen, narrow]);

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

  useEffect(() => {
    onRunningChange();
  }, [onRunningChange, running]);

  const handleScroll = () => {
    const list = listRef.current;
    if (list) {
      followTailRef.current =
        list.scrollHeight - list.scrollTop - list.clientHeight <= 48;
      setAtTail(followTailRef.current);
    }
  };
  const scrollToLatest = () => {
    followTailRef.current = true;
    setAtTail(true);
    const list = listRef.current;
    if (list) list.scrollTo({ top: list.scrollHeight, behavior: "auto" });
  };
  const resolveApproval = (decision: ApprovalDecisionValue) => {
    void controller
      .resolveApproval(decision)
      .catch((error) => onError(formatRuntimeError(error)));
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
  const handleBarTabClick = (tabId: InspectorTab) => {
    if (inspectorOpen && activeInspectorTab === tabId) {
      setInspectorOpen(false);
    } else {
      setActiveInspectorTab(tabId);
      layout.closeNavigation();
      setInspectorOpen(true);
    }
  };
  const stop = () => {
    void controller.stop();
  };
  const compactNow = () => {
    void controller
      .compact()
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
      <div className="chat-main">
      <ChatToolbar
        agentName={agent.name}
        title={title}
        running={running}
        temporary={isTest}
        ready={ready}
        clearing={clearingTest}
        inspectorOpen={inspectorOpen}
        activeTab={activeInspectorTab}
        hasDetail={Boolean(activeChip)}
        onTabClick={handleBarTabClick}
        onInspectorToggle={() => setInspectorOpen((open) => !open)}
        onBack={onBack}
        onNewSession={
          !isTest && onNewSession
            ? () =>
                void onNewSession(
                  running ? undefined : (sessionId ?? undefined),
                )
            : undefined
        }
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
                    在这里验证 <span className="mono">{agent.name}</span>{" "}
                    的已保存配置。
                    测试上下文只保留在本次应用运行期，不会进入会话历史。
                  </>
                ) : (
                  <>
                    <AgentAvatar
                      name={agent.name}
                      subagent={agent.subagent}
                      className="welcome-avatar"
                    />
                    <span className="welcome-kicker">{agent.name}</span>
                    <h2>
                      {locale === "en"
                        ? "What are we working on?"
                        : "这次想完成什么？"}
                    </h2>
                    <p>
                      {locale === "en"
                        ? "Share your goal. Your Agent will take it from here."
                        : "从一个目标开始，让 Agent 帮你向前推进。"}
                    </p>
                  </>
                )}
              </div>
            )}
            <ChatTimeline
              agent={agent}
              connecting={connecting}
              blocks={blocks}
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
            approval={pendingApproval}
            composingRef={composingRef}
            onSend={(text) => controller.send(text)}
            onBeforeSend={() => {
              followTailRef.current = true;
              setAtTail(true);
            }}
            onStop={stop}
            onResolveApproval={resolveApproval}
            onOpenModel={() => setModelModalOpen(true)}
            onScrollToLatest={scrollToLatest}
            onError={onError}
          />
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

        {renderInspector ? (
          <aside
            ref={inspectorRef}
            inert={!inspectorOpen}
            aria-hidden={!inspectorOpen || undefined}
            className="inspector agent-settings-inspector"
            id="session-inspector"
            aria-label="Agent 设置与测试信息"
          >
            {narrow && (
              <button
                type="button"
                className="icon-btn agent-settings-close"
                onClick={() => setInspectorOpen(false)}
                aria-label="关闭 Agent 设置"
                title="关闭 Agent 设置"
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
            onOpenFile={onOpenFile}
            hidden={!inspectorOpen}
            rootRef={inspectorRef}
            activeTab={activeInspectorTab}
            onTabChange={setActiveInspectorTab}
            hideHeaderTabs={true}
            onClose={() => setInspectorOpen(false)}
            searchQuery={searchQuery}
            onSearchQueryChange={setSearchQuery}
            searchHits={searchHits}
            pinnedFindKey={pinnedFindKey}
            previewFindKey={previewFindKey}
            onPreviewFind={setPreviewFindKey}
            onPinFind={(key) => {
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
          aria-label="关闭检查器"
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
