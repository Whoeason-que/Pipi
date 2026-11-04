import type { KeyboardEvent as ReactKeyboardEvent, RefObject } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
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
import SubagentPanel from "./SubagentPanel";
import { useNotebook } from "./notebook";
import { useI18n } from "../i18n";
// ============ 右侧检查器 ============

export interface InspectorProps {
  hidden?: boolean;
  rootRef?: RefObject<HTMLElement | null>;
  /** 会话投影快照（见 `inspector-context`）：正式会话与设置工作台的测试页共用。 */
  context: ChatInspectorContext;
  /** 手动压缩当前会话（「状态」tab 的「立即压缩」按钮）。 */
  onCompact: () => void;
  onOpenFile?: (path: string) => void;
  embedded?: boolean;
  allowCompact?: boolean;
  temporary?: boolean;
  activeTab?: InspectorTab;
  onTabChange?: (tab: InspectorTab) => void;
  /** 标签形态：text = 文字标签（嵌入式工作台），icon = 图标标签 + 关闭（会话右栏）。 */
  tabStyle?: "text" | "icon";
  onClose?: () => void;
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
  const [internalTab, setInternalTab] = useState<InspectorTab>("stats");
  const tab = activeTabProp ?? internalTab;
  const setTab = (next: InspectorTab) => {
    setInternalTab(next);
    onTabChange?.(next);
  };
  // 临时测试会话没有真实 notebook（id 属于进程内的测试账本），不占一个空 tab；
  // Subagent 预览属于正式会话的运行视图，也不放进调试工作台。
  const tabs = embedded
    ? INSPECTOR_TABS.filter((item) => item.id !== "notebook" && item.id !== "subagents")
    : INSPECTOR_TABS;
  const notebook = useNotebook(agent.name, embedded ? null : sessionId, entries);
  // 正在压缩：压缩条目在收尾前是 transient（见 chat-runtime 的 compaction_start）
  const compacting = entries.some(
    (entry) => entry.kind === "compaction" && entry.transient,
  );
  const canCompact =
    allowCompact && Boolean(sessionId) && !running && !compacting;
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
    const index = tabs.findIndex((item) => item.id === tab);
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const next =
      tabs[(index + delta + tabs.length) % tabs.length];
    setTab(next.id);
    document.getElementById(`itab-${next.id}`)?.focus();
  };

  const toolSummary = useMemo(() => {
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
  }, [entries]);

  const maxToolCount = toolSummary.reduce(
    (max, item) => Math.max(max, item.count),
    0,
  );
  const workspace = agent.workspace ?? `~/.pipi/agents/${agent.name}/workspace`;

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
          <div
            className="inspector-tabs"
            role="tablist"
            aria-label={t("inspector.viewsLabel")}
            onKeyDown={handleTabKeys}
          >
            {tabs.map((item) => {
              const label = getInspectorTabLabel(item.id, t);
              return (
                <button
                  key={item.id}
                  type="button"
                  role="tab"
                  id={`itab-${item.id}`}
                  aria-selected={tab === item.id}
                  aria-controls={`ipanel-${item.id}`}
                  aria-label={label}
                  title={label}
                  tabIndex={tab === item.id ? 0 : -1}
                  className={`inspector-tab${tab === item.id ? " active" : ""}`}
                  onClick={() => setTab(item.id)}
                >
                  <InspectorTabIcon
                    tab={item.id}
                    className="inspector-tab-icon"
                  />
                  {item.id === "detail" && chipDetail && tab !== "detail" && (
                    <span className="tab-dot" />
                  )}
                  {item.id === "notebook" &&
                    notebook.content &&
                    tab !== "notebook" && <span className="tab-dot" />}
                </button>
              );
            })}
          </div>
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
        <div
          className="itabs"
          role="tablist"
          aria-label={t("inspector.viewsLabel")}
          onKeyDown={handleTabKeys}
        >
          {tabs.map((item) => (
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
              {getInspectorTabLabel(item.id, t)}
            </button>
          ))}
        </div>
      )}

      <div className="ipanels">
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
          className={`ipanel${tab === "stats" ? " active" : ""}`}
          id="ipanel-stats"
          role="tabpanel"
          aria-labelledby="itab-stats"
        >
          <div className="ip context-stats">
            <h4>{t("inspector.currentContext")}</h4>
            {stats?.contextPercent != null || stats?.cacheHitPct != null ? (
              <>
                {stats?.contextPercent != null && (
                  <div className="context-meter">
                    <span className="m-k">{t("inspector.contextUsage")}</span>
                    <strong className="context-percent">
                      {Math.round(stats.contextPercent * 10) / 10}
                      <small>%</small>
                    </strong>
                    <span className="caption context-usage mono">
                      {formatTokens(stats.contextUsed)} /{" "}
                      {formatTokens(stats.contextMax)}
                    </span>
                    <span
                      className="track"
                      title={t("inspector.contextUsageHint")}
                    >
                      <i
                        style={{
                          width: `${Math.min(100, Math.max(0, stats.contextPercent))}%`,
                        }}
                      />
                    </span>
                  </div>
                )}
                {stats?.cacheHitPct != null && (
                  <div className="metric cache-metric">
                    <span className="m-k">{t("inspector.cacheHit")}</span>
                    <span className="m-v">{stats.cacheHitPct.toFixed(0)}%</span>
                    <span
                      className="track"
                      title={t("inspector.cacheHint")}
                    >
                      <i
                        className="cache"
                        style={{
                          width: `${Math.min(100, Math.max(0, stats.cacheHitPct))}%`,
                        }}
                      />
                    </span>
                  </div>
                )}
              </>
            ) : (
              <div className="ip-note">{t("inspector.noStats")}</div>
            )}
          </div>

          <div className="ip">
            <h4>{t("inspector.recentCalls")}</h4>
            {stats && (stats.avgTps != null || stats.avgLatencyS != null) ? (
              <div className="stat-pair">
                {stats.avgTps != null && (
                  <div className="stat-number">
                    <span className="m-k">{t("inspector.outputSpeed")}</span>
                    <span className="m-v">
                      {stats.avgTps.toFixed(1)}
                      <small>tok/s</small>
                    </span>
                  </div>
                )}
                {stats.avgLatencyS != null && (
                  <div className="stat-number">
                    <span className="m-k">{t("inspector.avgLatency")}</span>
                    <span className="m-v">
                      {stats.avgLatencyS.toFixed(2)}
                      <small>s</small>
                    </span>
                  </div>
                )}
              </div>
            ) : (
              <div className="ip-note">{t("inspector.notEnoughCalls")}</div>
            )}
          </div>

          <div className="ip">
            <h4>{t("inspector.cumulative")}</h4>
            <div className="metric">
              <span className="m-k">{t("inspector.calls")}</span>
              <span className="m-v">{stats?.calls ?? 0}</span>
            </div>
            <div className="metric">
              <span className="m-k">{t("inspector.inputOutput")}</span>
              <span className="m-v dim">
                {formatTokens(stats?.input)} / {formatTokens(stats?.output)}
              </span>
            </div>
          </div>

          <div className="ip">
            <h4>{t("inspector.toolCalls")}</h4>
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
                      {item.errors > 0
                        ? t("inspector.toolCountErrors", {
                            count: item.count,
                            errors: item.errors,
                          })
                        : t("inspector.toolCount", { count: item.count })}
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <div className="ip-note">{t("inspector.noToolCalls")}</div>
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
            <h4>{t("inspector.runtimeState")}</h4>
            <div className="kv-row">
              <span className="k">{runningTool ? t("inspector.currentTool") : t("inspector.lastTool")}</span>
              <span className="v">
                {runningTool || lastTool ? (
                  <>
                    <span className="mono" style={{ color: "var(--fg)" }}>
                      {(runningTool ?? lastTool)!.toolName ?? "tool"}
                    </span>{" "}
                    <span className="dim">
                      {runningTool
                        ? t("common.running")
                        : lastTool!.isError
                          ? t("common.failed")
                          : t("chat.statusDone")}
                    </span>
                  </>
                ) : (
                  <span className="dim">—</span>
                )}
              </span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.messageCount")}</span>
              <span className="v mono">{entries.length}</span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.calls")}</span>
              <span className="v mono">{stats?.calls ?? 0}</span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.blockedSessions")}</span>
              <span className="v mono dim">{blockedCount}</span>
            </div>
          </div>

          <div className="ip">
            <h4>{t("inspector.session")}</h4>
            <div className="kv-row">
              <span className="k">{t("inspector.model")}</span>
              <span className="v mono">
                {sessionModel?.id || t("inspector.notConfigured")}
                {sessionModel && (
                  <span className="dim">
                    {" "}
                    {isCustomModel ? t("inspector.custom") : t("inspector.default")}
                  </span>
                )}
              </span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.sessionId")}</span>
              <span className="v mono">{sessionId ?? t("inspector.newSession")}</span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.context")}</span>
              <span className="v mono">
                {stats?.contextPercent != null ? (
                  <>
                    {formatTokens(stats.contextUsed)} /{" "}
                    {formatTokens(stats.contextMax)}{" "}
                    <span className="dim">{stats.contextPercent}%</span>
                  </>
                ) : (
                  <span className="dim">—</span>
                )}
                {allowCompact && (
                  <button
                    type="button"
                    className="compact-now"
                    onClick={onCompact}
                    disabled={!canCompact}
                    title={t("inspector.compactHint")}
                  >
                    {compacting ? t("inspector.compacting") : t("inspector.compactNow")}
                  </button>
                )}
              </span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.sessionStorage")}</span>
              <span className="v mono dim">
                {temporary
                  ? t("inspector.memoryNoPersist")
                  : sessionId
                    ? `~/.pipi/agents/${agent.name}/sessions.git/`
                    : "—"}
              </span>
            </div>
          </div>

          <div className="ip">
            <h4>{t("inspector.constraints")}</h4>
            <div className="kv-row">
              <span className="k">{t("inspector.sandbox")}</span>
              <span className="v">
                <span className="tag ok">{agent.permissions.sandbox}</span>
              </span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.workingDirectory")}</span>
              <span className="v mono">{workspace}</span>
            </div>
            <div className="kv-row">
              <span className="k">{t("inspector.tools")}</span>
              <span className="v mono dim">
                {agent.permissions.tools.length
                  ? agent.permissions.tools.join(" · ")
                  : t("inspector.none")}
              </span>
            </div>
          </div>
        </div>

        <div
          className={`ipanel${tab === "subagents" ? " active" : ""}`}
          id="ipanel-subagents"
          role="tabpanel"
          aria-labelledby="itab-subagents"
        >
          <SubagentPanel
            agent={agent}
            sessionId={sessionId}
            entries={entries}
            parentRunning={running}
            runtime={runtime}
            active={tab === "subagents"}
          />
        </div>

        <div
          className={`ipanel${tab === "files" ? " active" : ""}`}
          id="ipanel-files"
          role="tabpanel"
          aria-labelledby="itab-files"
        >
          <SessionFilesPanel key={JSON.stringify([agent.name, sessionId])} entries={entries} onOpenFile={onOpenFile} />
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
