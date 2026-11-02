import assert from "node:assert/strict";
import test from "node:test";
import { filterQuickItems, quickItems } from "../src/app/navigation.ts";
import { cacheHitPct, emptyDay, fillDays, formatCost } from "../src/stats/format.ts";

test("cache hit rate uses prompt total and omits empty prompts", () => {
  assert.equal(cacheHitPct({ input: 100, cacheRead: 900, cacheWrite: 0 }), 90);
  assert.equal(cacheHitPct({ input: 0, cacheRead: 0, cacheWrite: 0 }), null);
});

test("cost formatting never fabricates a zero for unknown pricing", () => {
  assert.equal(formatCost(undefined), "—");
  assert.equal(formatCost(0), "$0");
  assert.equal(formatCost(0.0001), "<$0.01");
  assert.equal(formatCost(1.234), "$1.23");
});

test("fillDays pads a fixed range and preserves active days for all history", () => {
  const now = Date.UTC(2026, 10, 1, 12);
  const day = 86_400_000;
  const active = emptyDay(Math.floor(now / day) * day);
  active.tokens = 42;
  const filled = fillDays([active], 3, now);
  assert.equal(filled.length, 3);
  assert.equal(filled[2].tokens, 42);
  assert.equal(filled[0].tokens, 0);

  const all = fillDays([active], null, now);
  assert.equal(all.length, 1);
  assert.equal(all[0].tokens, 42);
});

test("stats is reachable from quick actions", () => {
  const items = quickItems([], {}, null);
  const stats = filterQuickItems(items, "Stats");
  assert.ok(stats.some((item) => item.id === "stats"));
});
