import type { KeyboardEvent as ReactKeyboardEvent, RefObject } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Tabs } from "radix-ui";
import { formatTokens } from "../chat-runtime";

import { IconClose, IconSearch } from "../icons";
import type { ChatInspectorContext } from "./inspector-context";
import { ChipDetailPane, formatClock } from "./ToolDetails";
import { extractSnippet, type SearchHit } from "./find";
import {
  INSPECTOR_TABS,
  InspectorTabIcon,
  getInspectorTabLabel,
  type InspectorTab,
} from "./inspector-tabs";
import { SessionFilesPanel } from "../files/SessionFilesPanel";
import { NotebookPanel } from "./NotebookPanel";
import SessionHistoryPanel from "./SessionHistoryPanel";
import SubagentPanel from "./SubagentPanel";
import { useNotebook } from "./notebook";
import { useI18n } from "../i18n";
import BrowserPanel from "./BrowserPanel";
import { TerminalPanel } from "../terminal/TerminalPanel";
import { isTauri } from "@tauri-apps/api/core";
// ============ 右侧检查器 ============

export interface InspectorProps {
  hidden?: boolean;
  rootRef?: RefObject<HTMLElement | null>;
  /** 会话投影快照（见 `inspector-context`）：正式会话与设置工作台的测试页共用。 */
  context: ChatInspectorContext;
  /** 手动摘要压缩当前会话（「状态」tab 的 Summarize 按钮）。 */
  onCompact: () => void;
  /** Agent 主导的编辑式压缩；嵌入式工作台不传。 */
  onAgentCompact?: () => void;
  onOpenFile?: (path: string) => void;
  embedded?: boolean;
  allowCompact?: boolean;
  temporary?: boolean;
  activeTab?: InspectorTab;
  onTabChange?: (tab: InspectorTab) => void;
  /** 标签形态：text = 文字标签（嵌入式工作台），icon = 图标标签 + 关闭（会话右栏）。 */
  tabStyle?: "text" | "icon";
  onClose?: () => void;
  ensureSession?: () => Promise<string>;
  searchQuery?: string;
  onSearchQueryChange?: (query: string) => void;
  searchHits?: SearchHit[];
  pinnedFindKey?: string | null;
  previewFindKey?: string | null;
  onPreviewFind?: (key: string | null) => void;
  onPinFind?: (key: string) => void;
  searchInputRef?: RefObject<HTMLInputElement | null>;
}

export default function Inspector({
  context,
  onCompact,
  onAgentCompact,
  onOpenFile,
  embedded = false,
  allowCompact = true,
  temporary = false,
  hidden = false,
  rootRef,
  activeTab: activeTabProp,
  onTabChange,
  tabStyle = "text",
  onClose,
  ensureSession,
  searchQuery = "",
  onSearchQueryChange,
  searchHits = [],
  pinnedFindKey = null,
  previewFindKey = null,
  onPreviewFind,
  onPinFind,
  searchInputRef,
}: InspectorProps) {
  const { t } = useI18n();
  const localSearchInputRef = useRef<HTMLInputElement>(null);
  const searchInput = searchInputRef ?? localSearchInputRef;
  const {
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
    runtime,
  } = context;
  const [internalTab, setInternalTab] = useState<InspectorTab>("state");
  const requestedTab = activeTabProp ?? internalTab;
  const tab = (requestedTab === "browser" && (embedded || !isTauri()))
    || (requestedTab === "terminal" && embedded)
    ? "state"
    : requestedTab;
  const setTab = (next: InspectorTab) => {
    setInternalTab(next);
    onTabChange?.(next);
  };
  // 临时测试会话没有真实 notebook（id 属于进程内的测试账本），不占一个空 tab；
  // git 历史属于正式会话的运行视图，也不放进调试工作台。
  const tabs = INSPECTOR_TABS.filter((item) =>
    !(embedded && (item.id === "notebook" || item.id === "history" || item.id === "browser" || item.id === "terminal")) &&
    !(item.id === "browser" && !isTauri()),
  );
  const notebook = useNotebook(agent.name, embedded ? null : sessionId, entries);
  // 正在压缩：压缩条目在收尾前是 transient（见 chat-runtime 的 compaction_start）
  const compacting = entries.some(
    (entry) => entry.kind === "compaction" && entry.transient,
  );
  const canCompact =
    allowCompact && Boolean(sessionId) && !running && !compacting;
  // Agent 编辑把完整历史塞进一次请求；超过 100k 的会话改用摘要更划算，
  // 后端也会再挡一次（这里只是提前禁用并给提示）。
  const agentCompactTooLarge = (stats?.contextUsed ?? 0) > 100_000;
  const canAgentCompact = canCompact && !agentCompactTooLarge;
  // 选中标签时自动切到「调用详情」，取消选中后回到之前的 tab
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const previousTabRef = useRef<InspectorTab>("state");
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

  useEffect(() => {
    if (tab === "search" && !hidden) {
      const timer = setTimeout(() => {
        searchInput.current?.focus();
      }, 50);
      return () => clearTimeout(timer);
    }
  }, [tab, hidden, searchInput]);

  const handleSearchKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (!searchHits || searchHits.length === 0) return;
    if (event.key === "Enter" || event.key === "ArrowDown") {
      event.preventDefault();
      const currentIndex = pinnedFindKey
        ? searchHits.findIndex((h) => h.key === pinnedFindKey)
        : -1;
      const nextIndex = (currentIndex + 1) % searchHits.length;
      onPinFind?.(searchHits[nextIndex].key);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      const currentIndex = pinnedFindKey
        ? searchHits.findIndex((h) => h.key === pinnedFindKey)
        : -1;
      const prevIndex =
        (currentIndex - 1 + searchHits.length) % searchHits.length;
      onPinFind?.(searchHits[prevIndex].key);
    }
  };

  // 这三个投影只被「状态」面板消费；其他 tab（以及收起的右栏）不必为每条
  // 流式 entries 反复全量扫描。
  const statePanelVisible = tab === "state" && !hidden;

  const lastTool = useMemo(() => {
    if (!statePanelVisible) return null;
    for (let index = entries.length - 1; index >= 0; index -= 1) {
      if (entries[index].role === "toolResult") return entries[index];
    }
    return null;
  }, [entries, statePanelVisible]);

  // 「当前工具」只在真的有工具在跑时才算数；否则只显示最近一次工具。
  const runningTool = useMemo(() => {
    if (!statePanelVisible) return null;
    for (let index = entries.length - 1; index >= 0; index -= 1) {
      const entry = entries[index];
      if (entry.role === "toolResult" && entry.toolRunning) return entry;
    }
    return null;
  }, [entries, statePanelVisible]);

  const toolSummary = useMemo(() => {
    if (!statePanelVisible) return [];
    const map = new Map<
      string,
      { count: number; errors: number; running: number }
    >();
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
  }, [entries, statePanelVisible]);

  const maxToolCount = toolSummary.reduce(
    (max, item) => Math.max(max, item.count),
    0,
  );
  const workspace = agent.workspace ?? `~/.pipi/agents/${agent.name}/workspace`;
  const contextPercent = stats?.contextPercent ?? null;
  const contextTrackPct = Math.min(100, Math.max(0, contextPercent ?? 0));
  const contextTone =
    contextPercent == null
      ? ""
      : contextPercent >= 90
        ? "danger"
        : contextPercent >= agent.compactThresholdPercent
          ? "warn"
          : "";
  const sandboxTone = agent.permissions.sandbox === "danger-full-access" ? "error" : "ok";
  const lastToolEntry = runningTool ?? lastTool;

  const Wrapper: "aside" | "div" = embedded ? "div" : "aside";
  return (
    <Wrapper
      ref={(element) => {
        if (rootRef) rootRef.current = element;
      }}
      inert={hidden}
      aria-hidden={hidden || undefined}
      className={embedded ? "embedded-inspector" : "inspector"}
      id={embedded ? undefined : "session-inspector"}
      aria-label={t(embedded ? "inspector.temporaryAria" : "inspector.ariaLabel")}
    >
      {tabStyle === "icon" ? (
        <div className="inspector-head inspector-head-tabs">
          <Tabs.Root className="tablist-host" value={tab} onValueChange={(next) => setTab(next as InspectorTab)}>
          <Tabs.List
            className="inspector-tabs"
            aria-label={t("inspector.viewsLabel")}
          >
            {tabs.map((item) => {
              const label = getInspectorTabLabel(item.id, t);
              return (
                <Tabs.Trigger
                  key={item.id}
                  value={item.id}
                  id={`itab-${item.id}`}
                  aria-controls={`ipanel-${item.id}`}
                  aria-label={label}
                  title={label}
                  className={`inspector-tab${tab === item.id ? " active" : ""}`}
                >
                  <InspectorTabIcon
                    tab={item.id}
                    className="inspector-tab-icon"
                  />
                  <span className="inspector-tab-label">{label}</span>
                  {item.id === "detail" && chipDetail && tab !== "detail" && (
                    <span className="tab-dot" />
                  )}
                  {item.id === "notebook" &&
                    notebook.content &&
                    tab !== "notebook" && <span className="tab-dot" />}
                </Tabs.Trigger>
              );
            })}
          </Tabs.List>
          </Tabs.Root>
          {onClose && (
            <button
              type="button"
              className="icon-btn inspector-close"
              onClick={onClose}
              aria-label={t("inspector.close")}
              title={t("inspector.close")}
            >
              <IconClose />
            </button>
          )}
        </div>
      ) : (
        <Tabs.Root className="tablist-host" value={tab} onValueChange={(next) => setTab(next as InspectorTab)}>
          <Tabs.List
            className="itabs"
            aria-label={t("inspector.viewsLabel")}
          >
            {tabs.map((item) => (
              <Tabs.Trigger
                key={item.id}
                value={item.id}
                id={`itab-${item.id}`}
                aria-controls={`ipanel-${item.id}`}
                className={`itab${tab === item.id ? " active" : ""}`}
              >
                {getInspectorTabLabel(item.id, t)}
              </Tabs.Trigger>
            ))}
          </Tabs.List>
        </Tabs.Root>
      )}

      <div className="ipanels">
        <div className={`ipanel browser-ipanel${tab === "browser" ? " active" : ""}`}
          id="ipanel-browser" role="tabpanel" aria-labelledby="itab-browser">
          {!embedded && isTauri() && <BrowserPanel active={tab === "browser" && !hidden} agentName={agent.name} sessionId={sessionId} ensureSession={ensureSession} />}
        </div>
        <div
          className={`ipanel terminal-ipanel${tab === "terminal" ? " active" : ""}`}
          id="ipanel-terminal"
          role="tabpanel"
          aria-labelledby="itab-terminal"
        >
          {!embedded && (
            <TerminalPanel
              agentName={agent.name}
              active={tab === "terminal" && !hidden}
              variant="rail"
            />
          )}
        </div>
        <div
          className={`ipanel${tab === "notebook" ? " active" : ""}`}
          id="ipanel-notebook"
          role="tabpanel"
          aria-labelledby="itab-notebook"
        >
          <NotebookPanel notebook={notebook} agent={agent} />
        </div>
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
            <div className="empty-state detail-empty">
              <span className="icon-tile inspector-empty-icon">
                <InspectorTabIcon tab="detail" />
              </span>
              <strong>{t("inspector.detailTitle")}</strong>
              <p>{t("inspector.noDetail")}</p>
            </div>
          )}
        </div>
        <div
          className={`ipanel${tab === "state" ? " active" : ""}`}
          id="ipanel-state"
          role="tabpanel"
          aria-labelledby="itab-state"
        >
          <div className="state-panel">
            <section className="state-card">
              <div className="state-card-head">
                <h4>{t("inspector.context")}</h4>
                <span className={`state-context-percent${contextTone ? ` ${contextTone}` : ""}`}>
                  {contextPercent != null ? `${contextPercent}%` : "—"}
                </span>
              </div>
              <div className="state-context-value">
                <strong>{contextPercent != null ? formatTokens(stats?.contextUsed) : "—"}</strong>
                {contextPercent != null && (
                  <span>/ {formatTokens(stats?.contextMax)}</span>
                )}
              </div>
              <div className="state-context-track" aria-hidden="true">
                <i className={contextTone} style={{ width: `${contextTrackPct}%` }} />
              </div>
              {allowCompact && (
                <div className="state-context-actions">
                  <button
                    type="button"
                    className="compact-now"
                    onClick={onCompact}
                    disabled={!canCompact}
                    title={t("inspector.compactHint")}
                  >
                    {compacting ? t("inspector.compacting") : t("inspector.compactNow")}
                  </button>
                  {onAgentCompact && (
                    <button
                      type="button"
                      className="compact-now agent-compact"
                      onClick={onAgentCompact}
                      disabled={!canAgentCompact}
                      title={
                        agentCompactTooLarge
                          ? t("inspector.agentCompactTooLarge")
                          : t("inspector.agentCompactHint")
                      }
                    >
                      {compacting ? t("inspector.compacting") : t("inspector.agentCompact")}
                    </button>
                  )}
                </div>
              )}
            </section>

            <section className="state-card">
              <h4>{t("inspector.stats")}</h4>
              {stats?.cacheHitPct != null && (
                <div className="state-cache">
                  <div className="state-cache-head">
                    <span>{t("inspector.cacheHit")}</span>
                    <strong>{stats.cacheHitPct.toFixed(0)}%</strong>
                  </div>
                  <div className="state-cache-track" title={t("inspector.cacheHint")}>
                    <i
                      style={{ width: `${Math.min(100, Math.max(0, stats.cacheHitPct))}%` }}
                    />
                  </div>
                </div>
              )}
              {stats && (stats.avgTps != null || stats.avgLatencyS != null) ? (
                <div className="state-stat-grid">
                  {stats.avgTps != null && (
                    <div className="state-stat">
                      <span className="state-stat-label">{t("inspector.outputSpeed")}</span>
                      <span className="state-stat-value">
                        {stats.avgTps.toFixed(1)}
                        <small>tok/s</small>
                      </span>
                    </div>
                  )}
                  {stats.avgLatencyS != null && (
                    <div className="state-stat">
                      <span className="state-stat-label">{t("inspector.avgLatency")}</span>
                      <span className="state-stat-value">
                        {stats.avgLatencyS.toFixed(2)}
                        <small>s</small>
                      </span>
                    </div>
                  )}
                </div>
              ) : (
                <div className="ip-note state-stat-empty">{t("inspector.notEnoughCalls")}</div>
              )}
              <div className="state-io">
                <span>{t("inspector.inputOutput")}</span>
                <span>
                  {formatTokens(stats?.input)} / {formatTokens(stats?.output)}
                </span>
              </div>
            </section>

            <section className="state-card">
              <h4>{t("inspector.runtimeState")}</h4>
              <div className="state-metrics">
                <div className="state-metric">
                  <span className="state-metric-label">{t("inspector.messageCount")}</span>
                  <span className="state-metric-value">{entries.length}</span>
                </div>
                <div className="state-metric">
                  <span className="state-metric-label">{t("inspector.calls")}</span>
                  <span className="state-metric-value">{stats?.calls ?? 0}</span>
                </div>
                <div className="state-metric">
                  <span className="state-metric-label">{t("inspector.blockedSessions")}</span>
                  <span className="state-metric-value">{blockedCount}</span>
                </div>
              </div>
              <div className="state-tool-line">
                <span className="state-tool-label">
                  {runningTool ? t("inspector.currentTool") : t("inspector.lastTool")}
                </span>
                <span className="state-tool-value">
                  {lastToolEntry ? (
                    <>
                      <span className="state-tool-name" title={lastToolEntry.toolName ?? "tool"}>
                        {lastToolEntry.toolName ?? "tool"}
                      </span>
                      <span
                        className={`state-tool-status ${
                          runningTool ? "running" : lastToolEntry.isError ? "failed" : "done"
                        }`}
                      >
                        {runningTool
                          ? t("common.running")
                          : lastToolEntry.isError
                            ? t("common.failed")
                            : t("chat.statusDone")}
                      </span>
                    </>
                  ) : (
                    <span className="dim">—</span>
                  )}
                </span>
              </div>
            </section>

            <section className="state-card">
              <h4>{t("inspector.session")}</h4>
              <div className="state-model-row">
                <span className="state-field-label">{t("inspector.model")}</span>
                <span className={`state-model-value${sessionModel ? "" : " dim"}`}>
                  <span className="mono">{sessionModel?.id || t("inspector.notConfigured")}</span>
                  {sessionModel && (
                    <span className="state-badge">
                      {isCustomModel ? t("inspector.custom") : t("inspector.default")}
                    </span>
                  )}
                </span>
              </div>
              <div className="state-meta-grid">
                <div className="state-field">
                  <span className="state-field-label">{t("inspector.sessionId")}</span>
                  <span className="state-field-value mono">
                    {sessionId ?? t("inspector.newSession")}
                  </span>
                </div>
                <div className="state-field">
                  <span className="state-field-label">{t("inspector.sessionStorage")}</span>
                  <span className="state-field-value mono dim">
                    {temporary
                      ? t("inspector.memoryNoPersist")
                      : sessionId
                        ? `~/.pipi/agents/${agent.name}/sessions.git/`
                        : "—"}
                  </span>
                </div>
              </div>
            </section>

            <section className="state-card">
              <h4>{t("inspector.constraints")}</h4>
              <div className="state-sandbox-row">
                <span className="state-field-label">{t("inspector.sandbox")}</span>
                <span className={`tag ${sandboxTone}`}>{agent.permissions.sandbox}</span>
              </div>
              <div className="state-field">
                <span className="state-field-label">{t("inspector.workingDirectory")}</span>
                <span className="state-field-value mono">{workspace}</span>
              </div>
              <div className="state-field state-field-tools">
                <span className="state-field-label">{t("inspector.tools")}</span>
                {agent.permissions.tools.length ? (
                  <div className="state-tool-chips">
                    {agent.permissions.tools.map((tool) => (
                      <span className="state-tool-chip" key={tool}>{tool}</span>
                    ))}
                  </div>
                ) : (
                  <span className="state-field-value dim">{t("inspector.none")}</span>
                )}
              </div>
            </section>

            <section className="state-card">
              <h4>{t("inspector.toolCalls")}</h4>
              {toolSummary.length > 0 ? (
                <div className="state-call-list">
                  {toolSummary.map((item) => (
                    <div className="state-call" key={item.name}>
                      <div className="state-call-top">
                        <span className="state-call-name" title={item.name}>
                          {item.name}
                        </span>
                        <span className={`state-call-count${item.errors > 0 ? " fail" : ""}`}>
                          {item.errors > 0
                            ? t("inspector.toolCountErrors", {
                                count: item.count,
                                errors: item.errors,
                              })
                            : t("inspector.toolCount", { count: item.count })}
                        </span>
                      </div>
                      <div className="state-call-bar" aria-hidden="true">
                        <i
                          style={{
                            width: `${maxToolCount > 0 ? Math.max(6, (item.count / maxToolCount) * 100) : 0}%`,
                          }}
                        />
                      </div>
                    </div>
                  ))}
                </div>
              ) : (
                <div className="ip-note">{t("inspector.noToolCalls")}</div>
              )}
            </section>
          </div>
        </div>

        <div
          className={`ipanel${tab === "history" ? " active" : ""}`}
          id="ipanel-history"
          role="tabpanel"
          aria-labelledby="itab-history"
        >
          <SubagentPanel
            agent={agent}
            sessionId={sessionId}
            entries={entries}
            parentRunning={running}
            runtime={runtime}
            active={tab === "history"}
          />
          <SessionHistoryPanel
            agent={agent}
            sessionId={sessionId}
            running={running}
            active={tab === "history"}
          />
        </div>

        <div
          className={`ipanel${tab === "files" ? " active" : ""}`}
          id="ipanel-files"
          role="tabpanel"
          aria-labelledby="itab-files"
        >
          <SessionFilesPanel key={JSON.stringify([agent.name, sessionId])} entries={entries} active={tab === "files" && !hidden} onOpenFile={onOpenFile} />
        </div>

        <div
          className={`ipanel${tab === "search" ? " active" : ""}`}
          id="ipanel-search"
          role="tabpanel"
          aria-labelledby="itab-search"
        >
          <div className="inspector-search-head">
            <div className="search-input-wrap">
              <IconSearch />
              <input
                ref={searchInput}
                type="text"
                value={searchQuery}
                onChange={(e) => onSearchQueryChange?.(e.target.value)}
                onKeyDown={handleSearchKeyDown}
                placeholder={t("inspector.searchPlaceholder")}
                aria-label={t("inspector.searchPlaceholder")}
                autoComplete="off"
                spellCheck={false}
              />
              {searchQuery && (
                <button
                  type="button"
                  className="search-clear-btn"
                  onClick={() => onSearchQueryChange?.("")}
                  aria-label={t("inspector.clearSearch")}
                  title={t("inspector.clearSearch")}
                >
                  <IconClose />
                </button>
              )}
            </div>
            <div className="caption search-stats-bar">
              <span className="search-stats-count">
                {searchQuery.trim()
                  ? t("inspector.searchResults", { count: searchHits.length })
                  : t("inspector.searchHint")}
              </span>
              {searchHits.length > 0 && (
                <span className="search-stats-hint">{t("inspector.searchResultHint")}</span>
              )}
            </div>
          </div>
          <div
            className="inspector-search-list"
            onMouseLeave={() => onPreviewFind?.(null)}
          >
            {!searchQuery.trim() ? (
              <div className="empty-state search-empty-state">
                <span className="icon-tile search-empty-icon">
                  <IconSearch />
                </span>
                <p className="search-empty-text">{t("inspector.searchEmpty")}</p>
                <span className="caption search-empty-kbd">
                  <kbd>Enter</kbd> / <kbd>↓</kbd> {t("inspector.searchNextHint")}
                </span>
              </div>
            ) : searchHits.length === 0 ? (
              <div className="empty-state search-empty-state">
                <p className="search-empty-text">
                  {t("inspector.noSearchResults")}
                </p>
              </div>
            ) : (
              searchHits.map((hit) => {
                const isPinned = pinnedFindKey === hit.key;
                const isPreview = previewFindKey === hit.key;
                const snippet = extractSnippet(hit.text, searchQuery);
                return (
                  <button
                    key={hit.key}
                    type="button"
                    className={`search-hit-card${isPinned ? " pinned" : ""}${isPreview ? " preview" : ""}`}
                    onPointerEnter={() => onPreviewFind?.(hit.key)}
                    onClick={() => onPinFind?.(hit.key)}
                    aria-selected={isPinned}
                  >
                    <div className="search-hit-top">
                      <span className={`search-hit-badge ${hit.role}`}>
                        {hit.role === "user" ? t("inspector.userRole") : t("inspector.assistantRole")}
                      </span>
                      {hit.timestamp ? (
                        <span className="caption search-hit-time">
                          {formatClock(hit.timestamp)}
                        </span>
                      ) : null}
                      <span className="spacer" />
                      {isPinned && (
                        <span className="search-hit-status pinned">{t("inspector.pinned")}</span>
                      )}
                      {!isPinned && isPreview && (
                        <span className="search-hit-status preview">
                          {t("inspector.previewing")}
                        </span>
                      )}
                    </div>
                    <div className="search-hit-text">
                      {snippet.prefix}
                      {snippet.match ? <mark>{snippet.match}</mark> : null}
                      {snippet.suffix}
                    </div>
                  </button>
                );
              })
            )}
          </div>
        </div>
      </div>
    </Wrapper>
  );
}
