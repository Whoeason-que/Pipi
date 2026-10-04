// 技能状态的展示口径（标签 / 色调 / 短哈希），技能库与 Agent 技能页共用。

import type { AgentSkillStatus, SkillVerifyStatus } from "../types";

export const AGENT_SKILL_STATUS_LABELS: Record<AgentSkillStatus, string> = {
  ready: "已就绪",
  missing: "内容缺失",
  broken: "内容不可解析",
  unresolved: "未解析",
  filtered: "被过滤",
};

export type SkillTone = "ok" | "warn" | "error" | "muted";

export const AGENT_SKILL_STATUS_TONES: Record<AgentSkillStatus, SkillTone> = {
  ready: "ok",
  missing: "error",
  broken: "error",
  unresolved: "warn",
  filtered: "muted",
};

/** 状态含义（悬浮提示用，与 av 的 fail-closed 判据对齐）。 */
export const AGENT_SKILL_STATUS_HINTS: Record<AgentSkillStatus, string> = {
  ready: "锁记录与 store 内容齐备，会话启动会注入这个技能",
  missing: "有解析锁但 store 里没有对应内容：同步可复现，未修复前会话启动会失败",
  broken: "内容在但 SKILL.md 缺失或不可解析：会话启动会失败",
  unresolved: "契约声明了 use，但同层 agent.lock 里没有解析记录：同步补齐",
  filtered: "被 only / exclude 过滤，不会进入系统提示",
};

export const SKILL_VERIFY_LABELS: Record<SkillVerifyStatus, string> = {
  ok: "一致",
  missing: "缺失",
  mismatch: "漂移",
};

export const SKILL_VERIFY_TONES: Record<SkillVerifyStatus, SkillTone> = {
  ok: "ok",
  missing: "error",
  mismatch: "error",
};

/** 内容哈希短标识（12 位 hex，与 av CLI 的展示口径一致）。 */
export function shortHash(hash: string | null | undefined): string {
  if (!hash) return "—";
  return hash.replace(/^sha256:/, "").slice(0, 12);
}

/** store 来源的一行描述。 */
export function sourceLabel(source: string, sourceType: string, revision?: string | null): string {
  if (sourceType === "local") return `本地 ${source}`;
  return revision ? `${source}@${revision.slice(0, 12)}` : source;
}

/** 色调 → 标签类名（`.tag.ok` / `.tag.warn` / `.tag.error` 均在 shell.css）。 */
export function toneClass(tone: SkillTone): string {
  if (tone === "ok") return "tag ok";
  if (tone === "warn") return "tag warn";
  if (tone === "error") return "tag error";
  return "tag";
}
