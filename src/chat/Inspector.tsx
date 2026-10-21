import type { KeyboardEvent as ReactKeyboardEvent, RefObject } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { formatTokens } from "../chat-runtime";

import { IconClose, IconSearch } from "../icons";
import { BASH_MODE_LABELS } from "../agents/agent-fields";
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
  hideHeaderTabs?: boolean;
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
  hideHeaderTabs = false,
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
  } = context;
  const [internalTab, setInternalTab] = useState<InspectorTab>("stats");
  const tab = activeTabProp ?? internalTab;
  const setTab = (next: InspectorTab) => {
    setInternalTab(next);
    onTabChange?.(next);
  };
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
    const index = INSPECTOR_TABS.findIndex((item) => item.id === tab);
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const next =
      INSPECTOR_TABS[
        (index + delta + INSPECTOR_TABS.length) % INSPECTOR_TABS.length
      ];
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
  const currentToolText = running
    ? runningTool
      ? `${runningTool.toolName ?? "tool"} · 运行中`
      : "模型调用中"
    : lastTool
      ? `上次工具 ${lastTool.toolName ?? "tool"}`
      : "尚无工具调用";

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
      aria-label={embedded ? "临时测试信息" : "会话检查器"}
    >
      {hideHeaderTabs ? (
        <div className="inspector-head">
          <span className="inspector-heading-icon">
            <InspectorTabIcon tab={tab} />
          </span>
          <span className="inspector-heading">
            <span className="inspector-title" id={`inspector-heading-${tab}`}>
              {getInspectorTabLabel(tab)}
            </span>
            <span className="inspector-subtitle">
              {agent.name}
              {temporary ? " · 临时测试" : " · 当前会话"}
            </span>
          </span>
          {onClose && (
            <button
              type="button"
              className="icon-btn"
              onClick={onClose}
              aria-label="关闭检查器"
              title="关闭检查器"
            >
              <IconClose />
            </button>
          )}
        </div>
      ) : (
        <div
          className="itabs"
          role="tablist"
          aria-label="检查器视图"
          onKeyDown={handleTabKeys}
        >
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
      )}

      <div className="ipanels">
        <div
          className={`ipanel${tab === "detail" ? " active" : ""}`}
          id="ipanel-detail"
          role="tabpanel"
          aria-labelledby={
            hideHeaderTabs ? "inspector-heading-detail" : "itab-detail"
          }
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
              <span className="inspector-empty-icon">
                <InspectorTabIcon tab="detail" />
              </span>
              <strong>查看调用过程</strong>
              <p>悬浮工具或思考标签以预览详情，点击后固定到这里。</p>
            </div>
          )}
        </div>
        <div
          className={`ipanel${tab === "stats" ? " active" : ""}`}
          id="ipanel-stats"
          role="tabpanel"
          aria-labelledby={
            hideHeaderTabs ? "inspector-heading-stats" : "itab-stats"
          }
        >
          <div className="ip context-stats">
            <h4>当前上下文</h4>
            {stats?.contextPercent != null || stats?.cacheHitPct != null ? (
              <>
                {stats?.contextPercent != null && (
                  <div className="context-meter">
                    <span className="m-k">上下文占用</span>
                    <strong className="context-percent">
                      {Math.round(stats.contextPercent * 10) / 10}
                      <small>%</small>
                    </strong>
                    <span className="context-usage mono">
                      {formatTokens(stats.contextUsed)} /{" "}
                      {formatTokens(stats.contextMax)}
                    </span>
                    <span
                      className="track"
                      title="最近一次请求的 prompt 用量 / 上下文窗口"
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
                    <span className="m-k">缓存命中</span>
                    <span className="m-v">{stats.cacheHitPct.toFixed(0)}%</span>
                    <span
                      className="track"
                      title="最近一次调用：cache_read / prompt"
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
              <div className="ip-note">暂无统计：本次会话还没有产生调用。</div>
            )}
          </div>

          <div className="ip">
            <h4>近 10 次调用</h4>
            {stats && (stats.avgTps != null || stats.avgLatencyS != null) ? (
              <div className="stat-pair">
                {stats.avgTps != null && (
                  <div className="stat-number">
                    <span className="m-k">输出速度</span>
                    <span className="m-v">
                      {stats.avgTps.toFixed(1)}
                      <small>tok/s</small>
                    </span>
                  </div>
                )}
                {stats.avgLatencyS != null && (
                  <div className="stat-number">
                    <span className="m-k">平均延迟</span>
                    <span className="m-v">
                      {stats.avgLatencyS.toFixed(2)}
                      <small>s</small>
                    </span>
                  </div>
                )}
              </div>
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
                      {item.count} 次
                      {item.errors > 0 ? ` · ${item.errors} 失败` : ""}
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
          aria-labelledby={
            hideHeaderTabs ? "inspector-heading-state" : "itab-state"
          }
        >
          <div className="ip">
            <h4>运行状态</h4>
            <div className={`state-line${running ? "" : " idle"}`}>
              <span className="dot" aria-hidden="true" />
              <strong>{running ? "正在运行" : "会话空闲"}</strong>
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
                      {runningTool
                        ? "运行中"
                        : lastTool!.isError
                          ? "失败"
                          : "完成"}
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
                {sessionModel && (
                  <span className="dim">
                    {" "}
                    {isCustomModel ? "自定义" : "默认"}
                  </span>
                )}
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
                    title="立即压缩当前会话（跳过阈值；走与自动压缩相同的分叉/归档设置）"
                  >
                    {compacting ? "压缩中…" : "立即压缩"}
                  </button>
                )}
              </span>
            </div>
            <div className="kv-row">
              <span className="k">会话存储</span>
              <span className="v mono dim">
                {temporary
                  ? "内存（不落盘）"
                  : sessionId
                    ? `~/.pipi/agents/${agent.name}/sessions.git/`
                    : "—"}
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
                {BASH_MODE_LABELS[agent.permissions.bash.mode] ??
                  agent.permissions.bash.mode}
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
                {agent.permissions.tools.length
                  ? agent.permissions.tools.join(" · ")
                  : "（无）"}
              </span>
            </div>
          </div>
        </div>

        <div
          className={`ipanel${tab === "files" ? " active" : ""}`}
          id="ipanel-files"
          role="tabpanel"
          aria-labelledby={
            hideHeaderTabs ? "inspector-heading-files" : "itab-files"
          }
        >
          <SessionFilesPanel key={JSON.stringify([agent.name, sessionId])} entries={entries} onOpenFile={onOpenFile} />
        </div>

        <div
          className={`ipanel${tab === "search" ? " active" : ""}`}
          id="ipanel-search"
          role="tabpanel"
          aria-labelledby={
            hideHeaderTabs ? "inspector-heading-search" : "itab-search"
          }
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
                placeholder="搜索当前会话消息…"
                aria-label="搜索当前会话消息"
                autoComplete="off"
                spellCheck={false}
              />
              {searchQuery && (
                <button
                  type="button"
                  className="search-clear-btn"
                  onClick={() => onSearchQueryChange?.("")}
                  aria-label="清空搜索"
                  title="清空搜索"
                >
                  <IconClose />
                </button>
              )}
            </div>
            <div className="search-stats-bar">
              <span className="search-stats-count">
                {searchQuery.trim()
                  ? `共找到 ${searchHits.length} 条匹配`
                  : "按消息正文检索"}
              </span>
              {searchHits.length > 0 && (
                <span className="search-stats-hint">悬浮预览 · 点击固定</span>
              )}
            </div>
          </div>
          <div
            className="inspector-search-list"
            onMouseLeave={() => onPreviewFind?.(null)}
          >
            {!searchQuery.trim() ? (
              <div className="search-empty-state">
                <span className="search-empty-icon">
                  <IconSearch />
                </span>
                <p className="search-empty-text">输入关键词检索当前会话消息</p>
                <span className="search-empty-kbd">
                  <kbd>Enter</kbd> / <kbd>↓</kbd> 切换下一条
                </span>
              </div>
            ) : searchHits.length === 0 ? (
              <div className="search-empty-state">
                <p className="search-empty-text">
                  未找到与 “{searchQuery}” 相关的消息
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
                        {hit.role === "user" ? "用户" : "助手"}
                      </span>
                      {hit.timestamp ? (
                        <span className="search-hit-time">
                          {formatClock(hit.timestamp)}
                        </span>
                      ) : null}
                      <span className="spacer" />
                      {isPinned && (
                        <span className="search-hit-status pinned">已固定</span>
                      )}
                      {!isPinned && isPreview && (
                        <span className="search-hit-status preview">
                          预览中
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
