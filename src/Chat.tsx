import type { ReactNode } from "react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import {
  formatRuntimeError,
  groupChatBlocks,
  type ApprovalDecisionValue,
  type Chip
} from "./chat-runtime";
import ChatTimeline from "./chat/ChatTimeline";
import Inspector from "./chat/Inspector";
import type { ChatInspectorContext } from "./chat/inspector-context";
import ModelSelectModal from "./chat/ModelSelectModal";
import { IconBack, IconFork, IconGrid, IconPlus, IconSend, IconStop } from "./icons";
import { useResizableWidth } from "./resizable";
import type { SessionRuntime } from "./session-runtime";
import type { AgentDefinition, ModelConfig, ProviderConfig, SessionInfoView } from "./types";

export type { MessageView } from "./chat-runtime";


interface ChatViewProps {
  agent: AgentDefinition;
  providers: ProviderConfig[];
  /** 正在查看的会话 id；null = 新会话（还没有文件，首次发送时创建）。 */
  sessionId: string | null;
  sessions: SessionRuntime;
  blockedSessionIds: string[];
  onBack: () => void;
  onError: (msg: string) => void;
  onNewSession: (previousSessionId?: string) => Promise<boolean>;
  onRunningChange: (agentName: string, sessionId: string | null, running: boolean) => void;
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
  agent,
  providers,
  sessionId: initialSessionId,
  sessions,
  blockedSessionIds,
  onBack,
  onError,
  onNewSession,
  onRunningChange,
  onSessionReset,
  mode = "session",
  onClearTest,
  renderInspector,
  onInspectDetail,
}: ChatViewProps) {
  const isTest = mode === "test";
  const controller = useMemo(() => sessions.get(agent, initialSessionId, isTest),
    [sessions, agent.name, initialSessionId, isTest]);
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const { entries, stats, running } = snapshot.chat;
  const ready = snapshot.load === "ready";
  const stopping = snapshot.operation === "stopping";
  const sessionId = snapshot.sessionId;
  const sessionModel = snapshot.model;
  const isCustomModel = snapshot.isCustomModel;
  const pendingApproval = snapshot.approval;
  const [input, setInput] = useState("");
  const [clearingTest, setClearingTest] = useState(false);
  const [pinnedChip, setPinnedChip] = useState<string | null>(null);
  const [previewChip, setPreviewChip] = useState<string | null>(null);
  const [modelModalOpen, setModelModalOpen] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(
    () => typeof window === "undefined" || window.innerWidth > 1100,
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
  const followTailRef = useRef(true);
  const composingRef = useRef(false);
  const compositionEndedAtRef = useRef(0);
  const lastErrorRef = useRef(0);
  const lastChangeRef = useRef(snapshot.change?.revision ?? 0);

  useEffect(() => { controller.updateAgent(agent); }, [controller, agent]);
  useEffect(() => {
    let active = true;
    mountedRef.current = true;
    void sessions.start().then(() => controller.hydrate()).catch(error => {
      if (active) onError(formatRuntimeError(error));
    });
    return () => { active = false; mountedRef.current = false; };
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
    setInspectorOpen(!narrow);
  }, [narrow]);

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

  useLayoutEffect(() => {
    const textarea = inputRef.current;
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }, [input]);

  useEffect(() => {
    onRunningChange(agent.name, sessionId, running);
  }, [agent.name, onRunningChange, sessionId, running]);

  const handleScroll = () => {
    const list = listRef.current;
    if (list) followTailRef.current = list.scrollHeight - list.scrollTop - list.clientHeight <= 48;
  };
  const send = async () => {
    const text = input.trim();
    if (!ready || !text) return;
    setInput("");
    followTailRef.current = true;
    try {
      if (!await controller.send(text) && mountedRef.current) setInput(text);
    } catch (error) {
      if (mountedRef.current) { setInput(text); onError(formatRuntimeError(error)); }
    }
  };
  const resolveApproval = (decision: ApprovalDecisionValue) => {
    void controller.resolveApproval(decision).catch(error => onError(formatRuntimeError(error)));
  };
  const previewDetail = (id: string) => setPreviewChip(id);
  const clearPreviewDetail = () => setPreviewChip(null);
  const togglePinnedDetail = (id: string) => {
    setPinnedChip(current => current === id ? null : id);
    setPreviewChip(null);
    onInspectDetail?.();
    if (narrow) setInspectorOpen(true);
  };
  const stop = () => { void controller.stop(); };
  const newSession = async () => {
    if (!ready || running) return;
    if (await onNewSession(sessionId ?? undefined) && mountedRef.current) {
      setInput("");
      followTailRef.current = true;
    }
  };
  const compactNow = () => {
    void controller.compact().catch(error => onError(formatRuntimeError(error)));
  };
  const forkSession = () => {
    void controller.fork().catch(error => onError(formatRuntimeError(error)));
  };
  const handleModelChange = async (target: { isCustom: boolean; model: ModelConfig | null }) => {
    await controller.setModel(target.isCustom, target.model);
    setModelModalOpen(false);
  };
  const clearTest = async () => {
    if (!isTest || !onClearTest || running || clearingTest) return;
    setClearingTest(true);
    try { await onClearTest(); }
    catch (error) { onError(formatRuntimeError(error)); }
    finally { if (mountedRef.current) setClearingTest(false); }
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
      style={{ "--inspector-width": `${inspectorResize.width}px` } as React.CSSProperties}
    >
      <div className="chat">
        <div className="screen-bar">
          <button type="button" className="icon-btn" title="返回" aria-label="返回" onClick={onBack}>
            <IconBack />
          </button>
          {isTest ? (
            <span className="crumb" title={`Agent 工作台 · ${agent.name}`}>
              Agent 工作台 · <b>{agent.name}</b>
            </span>
          ) : (
            <span className="crumb" title={`~/.pipi/agents/${agent.name}/sessions/${sessionId ?? ""}`}>
              ~/.pipi/agents/<b>{agent.name}</b>/sessions/{sessionId ? <b>{sessionId}</b> : "…"}
            </span>
          )}
          <span className="spacer" />
          {isTest ? (
            <>
              <span className="test-mode-pill">
                <span className={`test-mode-dot${running ? " running" : ""}`} />
                临时测试
              </span>
              <span className="test-model mono" title={sessionModel?.id || "未配置模型"}>
                {sessionModel?.id || "未配置模型"}
              </span>
              <button
                type="button"
                className="btn ghost test-clear"
                onClick={() => void clearTest()}
                disabled={!ready || running || clearingTest}
                title={running ? "请先停止测试" : "清空临时上下文"}
              >
                {clearingTest ? "清空中…" : "清空测试"}
              </button>
            </>
          ) : (
            <>
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
                disabled={!ready || running || !sessionId}
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
            </>
          )}
          <button
            type="button"
            className={`icon-btn${inspectorOpen ? " active" : ""}`}
            onClick={() => setInspectorOpen((open) => !open)}
            title={isTest ? "Agent 设置" : "统计 / 检查器"}
            aria-label={isTest ? "切换 Agent 设置" : "切换检查器"}
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
              {isTest ? (
                <>
                  在这里验证 <span className="mono">{agent.name}</span> 的已保存配置。
                  测试上下文只保留在本次应用运行期，不会进入会话历史。
                </>
              ) : (
                <>
                  与 <span className="mono">{agent.name}</span> 对话。会话记录将写入
                  <span className="mono"> sessions/*.jsonl</span>。
                </>
              )}
            </div>
          )}
          <ChatTimeline blocks={blocks} activeChipId={activeChipId} pinnedChip={pinnedChip}
            previewDetail={previewDetail} clearPreviewDetail={clearPreviewDetail} togglePinnedDetail={togglePinnedDetail} />
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
          {isTest && (
            <div className="test-safety-note" role="note">
              <span>临时仅指对话记录</span>
              工具仍按当前权限真实运行，对工作区造成的改动会保留。
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
                disabled={!ready || !input.trim()}
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

      {renderInspector ? (
        <aside
          className="inspector agent-settings-inspector"
          id="session-inspector"
          aria-label="Agent 设置与测试信息"
        >
          {renderInspector(inspectorContext)}
        </aside>
      ) : (
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
          onCompact={compactNow}
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
