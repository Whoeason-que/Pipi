// Agent 级统计页：跨会话汇总用量、成本、工具与活动趋势。
// 数据来自宿主命令 `agent_stats`（会话语义见 crates/pipi-core/src/stats.rs），
// 页面只做展示与筛选，不在前端重算成本/命中率等口径。
import { useEffect, useMemo, useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { formatTokens } from "../chat-runtime";
import AgentAvatar from "../components/AgentAvatar";
import { useI18n } from "../i18n";
import { IconChart, IconClock, IconRefresh } from "../icons";
import { invoke } from "../platform";
import { ChoiceSelect } from "../Select";
import type { AgentDefinition, AgentStatsView } from "../types";
import {
  cacheHitPct,
  dayLabel,
  fillDays,
  formatCost,
  formatCount,
  formatDuration,
  formatPercent,
} from "./format";

interface StatsViewProps {
  agent: AgentDefinition | null;
  agents: AgentDefinition[];
  onSelectAgent: (name: string) => void;
  onOpenSession: (agentName: string, sessionId: string) => void;
}

const RANGES: Array<{ value: number | null; label: string }> = [
  { value: 7, label: "7D" },
  { value: 30, label: "30D" },
  { value: 90, label: "90D" },
  { value: null, label: "ALL" },
];

export default function StatsView({
  agent,
  agents,
  onSelectAgent,
  onOpenSession,
}: StatsViewProps) {
  const { locale, t } = useI18n();
  const [rangeDays, setRangeDays] = useState<number | null>(30);
  const [stats, setStats] = useState<AgentStatsView | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    if (!agent) {
      setStats(null);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    setStats(null);
    invoke("agent_stats", { agentName: agent.name, rangeDays })
      .then((next) => {
        if (!cancelled) setStats(next);
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setError(reason instanceof Error ? reason.message : String(reason));
          setStats(null);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [agent?.name, rangeDays, refresh]);

  const choices = useMemo(
    () => agents.filter((item) => !item.subagent).map((item) => ({
      value: item.name,
      label: item.name,
    })),
    [agents],
  );
  const totals = stats?.totals;
  const hit = totals ? cacheHitPct(totals) : null;
  const hasData = Boolean(
    totals && (totals.assistantCalls > 0 || totals.userMessages > 0 || totals.toolCalls > 0),
  );
  const chartDays = useMemo(
    () => stats ? fillDays(stats.days, rangeDays, Date.now()).slice(-90) : [],
    [stats, rangeDays],
  );
  const maxTokens = chartDays.reduce((max, day) => Math.max(max, day.tokens), 0);
  const maxModelCalls = stats?.models.reduce((max, item) => Math.max(max, item.calls), 0) ?? 0;
  const maxToolCalls = stats?.tools.reduce((max, item) => Math.max(max, item.calls), 0) ?? 0;
  const dateTime = useMemo(
    () => new Intl.DateTimeFormat(locale === "zh" ? "zh-CN" : "en", {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }),
    [locale],
  );

  return (
    <div className="stats-view">
      <header className="workspace-toolbar">
        <NavigationToggle />
        <span className="toolbar-divider" />
        <span>{t("stats.title")}</span>
        <span className="spacer" />
        {agent && (
          <button
            type="button"
            className="icon-btn"
            title={t("common.refresh")}
            aria-label={t("common.refresh")}
            onClick={() => setRefresh((value) => value + 1)}
          >
            <IconRefresh />
          </button>
        )}
      </header>

      <div className="overview-page stats-page">
        <header className="overview-heading stats-heading">
          {agent && (
            <AgentAvatar
              name={agent.name}
              subagent={agent.subagent}
              className="overview-avatar"
            />
          )}
          <div className="overview-intro">
            <h1>{t("stats.title")}</h1>
            <p>{agent ? `${agent.name} · ${t("stats.subtitle")}` : t("stats.selectAgent")}</p>
            <p className="caption stats-primary-only">{t("stats.primaryOnly")}</p>
            {stats && (
              <div className="stats-generated">
                <IconClock />
                {t("stats.generatedAt", { time: dateTime.format(stats.generatedAt) })}
                {stats.sessionsTruncated && (
                  <span className="stats-warning">{t("stats.truncated")}</span>
                )}
              </div>
            )}
          </div>
          <div className="stats-controls">
            <ChoiceSelect
              value={agent?.name ?? ""}
              choices={choices}
              onChange={onSelectAgent}
              placeholder={t("stats.selectAgent")}
              ariaLabel={t("stats.selectAgent")}
              className="stats-agent-select"
            />
            <div className="stats-range" role="group" aria-label={t("stats.range")}>
              {RANGES.map((item) => (
                <button
                  key={item.label}
                  type="button"
                  className={`stats-range-btn${rangeDays === item.value ? " active" : ""}`}
                  aria-pressed={rangeDays === item.value}
                  onClick={() => setRangeDays(item.value)}
                >
                  {item.label}
                </button>
              ))}
            </div>
          </div>
        </header>

        {!agent && (
          <div className="overview-empty">
            <span className="icon-tile"><IconChart /></span>
            <h3>{t("stats.selectAgent")}</h3>
            <p>{t("stats.selectAgentHint")}</p>
          </div>
        )}

        {agent && error && (
          <div className="ip stats-error" role="alert">
            <strong>{t("stats.loadError")}</strong>
            <p>{error}</p>
            <button type="button" className="btn" onClick={() => setRefresh((value) => value + 1)}>
              {t("common.retry")}
            </button>
          </div>
        )}

        {agent && !error && loading && !stats && (
          <div className="session pending">{t("stats.loading")}</div>
        )}

        {agent && !error && stats && !hasData && (
          <div className="overview-empty compact">
            <span className="icon-tile"><IconChart /></span>
            <h3>{t("stats.empty")}</h3>
            <p>{t("stats.emptyHint")}</p>
          </div>
        )}

        {agent && !error && stats && hasData && totals && (
          <>
            <section className="stats-stat-grid" aria-label={t("stats.overview")}>
              <StatCard label={t("stats.sessions")} value={formatCount(totals.sessions)}
                detail={t("stats.userMessagesDetail", { count: totals.userMessages })} />
              <StatCard label={t("stats.calls")} value={formatCount(totals.assistantCalls)}
                detail={t("stats.errorsDetail", { count: totals.assistantErrors + totals.aborted })} />
              <StatCard label={t("stats.tokens")} value={formatTokens(totals.tokens)}
                detail={`${t("stats.input")} ${formatTokens(totals.input)} · ${t("stats.output")} ${formatTokens(totals.output)}`} />
              <StatCard label={t("stats.estimatedCost")} value={formatCost(totals.cost)}
                detail={totals.unpricedCalls > 0
                  ? t("stats.unpricedCalls", { count: totals.unpricedCalls })
                  : t("stats.costHint")} />
              <StatCard label={t("stats.cacheHit")} value={formatPercent(hit)}
                detail={t("stats.cacheDetail", {
                  read: formatTokens(totals.cacheRead),
                  write: formatTokens(totals.cacheWrite),
                })} />
              <StatCard label={t("stats.toolCalls")} value={formatCount(totals.toolCalls)}
                detail={t("stats.toolErrorsDetail", { count: totals.toolErrors })} />
              <StatCard label={t("stats.compactions")} value={formatCount(totals.compactions)}
                detail={t("stats.generationDetail", { duration: formatDuration(totals.generationMs) })} />
            </section>

            <section className="overview-section">
              <div className="section-heading">
                <h2>{t("stats.trend")}</h2>
                <span className="caption">{t("stats.trendHint")}</span>
              </div>
              <div className="surface stats-trend-card">
                {chartDays.length > 0 ? (
                  <>
                    <div className="stats-trend" role="img" aria-label={t("stats.trend")}>
                      {chartDays.map((day) => (
                        <div
                          key={day.dayStart}
                          className="stats-trend-col"
                          title={`${dayLabel(day.dayStart, locale)} · ${formatTokens(day.tokens)} tok · ${formatCount(day.assistantCalls)} ${t("stats.calls")}`}
                        >
                          <i style={{ height: `${maxTokens > 0 ? Math.max(2, (day.tokens / maxTokens) * 100) : 2}%` }} />
                        </div>
                      ))}
                    </div>
                    <div className="stats-trend-axis">
                      <span>{dayLabel(chartDays[0].dayStart, locale)}</span>
                      <span>{dayLabel(chartDays[chartDays.length - 1].dayStart, locale)}</span>
                    </div>
                  </>
                ) : (
                  <div className="ip-note">{t("stats.noTrend")}</div>
                )}
              </div>
            </section>

            <div className="stats-two-col">
              <section className="overview-section">
                <div className="section-heading">
                  <h2>{t("stats.models")}</h2>
                  <span className="caption">{t("stats.byCalls")}</span>
                </div>
                <div className="surface stats-list">
                  {stats.models.length === 0 && <div className="ip-note">{t("stats.noData")}</div>}
                  {stats.models.map((item) => (
                    <div className="stats-list-row" key={item.model}>
                      <span className="stats-list-name mono" title={item.model}>{item.model || t("common.unknown")}</span>
                      <span className="stats-bar">
                        <i style={{ width: `${maxModelCalls > 0 ? Math.max(4, (item.calls / maxModelCalls) * 100) : 0}%` }} />
                      </span>
                      <span className="stats-list-value">
                        {formatCount(item.calls)} · {formatTokens(item.tokens)}
                        {item.cost != null && <b>{formatCost(item.cost)}</b>}
                        {item.errors > 0 && <em>{t("stats.failedCount", { count: item.errors })}</em>}
                      </span>
                    </div>
                  ))}
                </div>
              </section>

              <section className="overview-section">
                <div className="section-heading">
                  <h2>{t("stats.tools")}</h2>
                  <span className="caption">{t("stats.byCalls")}</span>
                </div>
                <div className="surface stats-list">
                  {stats.tools.length === 0 && <div className="ip-note">{t("stats.noTools")}</div>}
                  {stats.tools.map((item) => (
                    <div className="stats-list-row" key={item.name}>
                      <span className="stats-list-name mono" title={item.name}>{item.name}</span>
                      <span className="stats-bar">
                        <i style={{ width: `${maxToolCalls > 0 ? Math.max(4, (item.calls / maxToolCalls) * 100) : 0}%` }} />
                      </span>
                      <span className="stats-list-value">
                        {formatCount(item.calls)}
                        {item.errors > 0 && <em>{t("stats.failedCount", { count: item.errors })}</em>}
                      </span>
                    </div>
                  ))}
                </div>
              </section>
            </div>

            <section className="overview-section">
              <div className="section-heading">
                <h2>{t("stats.recentSessions")}</h2>
                <span className="caption">{t("stats.sessionsHint")}</span>
              </div>
              <div className="surface stats-table-wrap">
                <table className="stats-table">
                  <thead>
                    <tr>
                      <th>{t("stats.session")}</th>
                      <th>{t("stats.model")}</th>
                      <th>{t("stats.calls")}</th>
                      <th>{t("stats.tokens")}</th>
                      <th>{t("stats.estimatedCost")}</th>
                      <th>{t("stats.tools")}</th>
                      <th>{t("stats.lastActive")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {stats.sessions.map((session) => (
                      <tr
                        key={session.id}
                        tabIndex={0}
                        role="link"
                        title={t("stats.openSession")}
                        onClick={() => onOpenSession(agent.name, session.id)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") onOpenSession(agent.name, session.id);
                        }}
                      >
                        <td>
                          <span className="stats-session-title">{session.title || t("workspace.untitled")}</span>
                          <span className="caption mono stats-session-id">{session.id}</span>
                        </td>
                        <td className="mono">{session.model || "—"}</td>
                        <td>{formatCount(session.assistantCalls)}</td>
                        <td>{formatTokens(session.tokens)}</td>
                        <td>{formatCost(session.cost)}{session.unpricedCalls > 0 ? " *" : ""}</td>
                        <td>{formatCount(session.toolCalls)}{session.toolErrors > 0 ? ` · ${session.toolErrors} ✕` : ""}</td>
                        <td className="caption">{dateTime.format(session.lastActive)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="caption stats-footnote">{t("stats.costFootnote")}</p>
            </section>
          </>
        )}
      </div>
    </div>
  );
}

function StatCard({ label, value, detail }: { label: string; value: string; detail: string }) {
  return (
    <div className="stats-stat">
      <span className="stats-stat-label">{label}</span>
      <strong className="stats-stat-value">{value}</strong>
      <span className="caption stats-stat-detail">{detail}</span>
    </div>
  );
}
