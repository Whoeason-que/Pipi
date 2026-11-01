import type { StatsDayView, StatsTotalsView } from "../types.ts";

const DAY_MS = 86_400_000;

/** 缓存命中率：命中 / 提示词总量（input 只计未命中部分）。没有提示词时返回 null。 */
export function cacheHitPct(totals: Pick<StatsTotalsView, "input" | "cacheRead" | "cacheWrite">): number | null {
  const prompt = totals.input + totals.cacheRead + totals.cacheWrite;
  if (prompt <= 0) return null;
  return (totals.cacheRead / prompt) * 100;
}

/** 成本口径统一：没有价格显示 `—`；小于 1 美分时显示 `<$0.01`。 */
export function formatCost(cost: number | undefined): string {
  if (cost == null) return "—";
  if (cost <= 0) return "$0";
  if (cost < 0.01) return "<$0.01";
  return `$${cost.toFixed(2)}`;
}

export function formatCount(value: number | undefined): string {
  return value == null ? "—" : value.toLocaleString();
}

export function formatPercent(value: number | null, digits = 1): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return `${value.toFixed(digits)}%`;
}

export function formatDuration(ms: number | undefined): string {
  if (ms == null || ms <= 0) return "—";
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}

/** 补齐趋势图的日期序列：固定范围补零，全部历史只保留有活动的日期。 */
export function fillDays(days: StatsDayView[], rangeDays: number | null, now: number): StatsDayView[] {
  if (rangeDays == null || rangeDays <= 0) {
    return [...days].sort((a, b) => a.dayStart - b.dayStart);
  }
  const byStart = new Map(days.map((day) => [day.dayStart, day]));
  const todayStart = Math.floor(now / DAY_MS) * DAY_MS;
  return Array.from({ length: rangeDays }, (_, index) => {
    const dayStart = todayStart - (rangeDays - 1 - index) * DAY_MS;
    return byStart.get(dayStart) ?? emptyDay(dayStart);
  });
}

export function emptyDay(dayStart: number): StatsDayView {
  return {
    dayStart,
    userMessages: 0,
    assistantCalls: 0,
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    tokens: 0,
    toolCalls: 0,
    toolErrors: 0,
    assistantErrors: 0,
    sessions: 0,
    unpricedCalls: 0,
  };
}

export function dayLabel(dayStart: number, locale: string): string {
  return new Intl.DateTimeFormat(locale === "zh" ? "zh-CN" : "en", {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  }).format(dayStart);
}
