import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  formatTokens,
  type ChatEntry,
  type Chip
} from "../chat-runtime";
import type { AgentDefinition, ModelConfig, SessionStatsView } from "../types";

import type { ChatInspectorContext } from "./inspector-context";
import { ChipDetailPane } from "./ToolDetails";
const WRITE_TOOLS = new Set(["write", "edit"]);
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
  /** 手动压缩当前会话（「状态」tab 的「立即压缩」按钮）。 */
  onCompact: () => void;
  embedded?: boolean;
  allowCompact?: boolean;
  temporary?: boolean;
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

export default function Inspector({
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
  onCompact,
  embedded = false,
  allowCompact = true,
  temporary = false,
}: InspectorProps) {
  const [tab, setTab] = useState<InspectorTab>("stats");
  // 正在压缩：压缩条目在收尾前是 transient（见 chat-runtime 的 compaction_start）
  const compacting = entries.some((entry) => entry.kind === "compaction" && entry.transient);
  const canCompact = allowCompact && Boolean(sessionId) && !running && !compacting;
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

  const Wrapper: "aside" | "div" = embedded ? "div" : "aside";
  return (
    <Wrapper
      className={embedded ? "embedded-inspector" : "inspector"}
      id={embedded ? undefined : "session-inspector"}
      aria-label={embedded ? "临时测试信息" : "会话检查器"}
    >
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
                <span className="k">会话文件</span>
                <span className="v mono dim">
                  {temporary
                    ? "内存（不落盘）"
                    : sessionId
                      ? `~/.pipi/agents/${agent.name}/sessions/`
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
    </Wrapper>
  );
}

/** 设置工作台「测试」页复用正式会话检查器的数据解释与展示。 */
export function SessionDiagnostics({ context }: { context: ChatInspectorContext }) {
  return (
    <Inspector
      agent={context.agent}
      sessionId={context.sessionId}
      entries={context.entries}
      stats={context.stats}
      running={context.running}
      sessionModel={context.sessionModel}
      isCustomModel={context.isCustomModel}
      blockedCount={context.blockedCount}
      chipDetail={context.chipDetail}
      chipDetailMode={context.chipDetailMode}
      onUnpinChip={context.onUnpinChip}
      onCompact={() => {}}
      embedded
      allowCompact={false}
      temporary
    />
  );
}
