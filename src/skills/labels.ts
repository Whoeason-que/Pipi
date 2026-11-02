// 技能状态的展示口径（标签 / 色调 / 短哈希），技能库与 Agent 技能页共用。

import type { AgentSkillStatus, SkillVerifyStatus } from "../types";

/** 翻译函数（与 useI18n().t 同形，供非组件模块复用）。 */
export type SkillTranslate = (path: string, params?: Record<string, string | number>) => string;

/** 状态标签的 i18n key（展示前经 t() 解析）。 */
export const AGENT_SKILL_STATUS_LABELS: Record<AgentSkillStatus, string> = {
  ready: "skills.status.ready",
  missing: "skills.status.missing",
  broken: "skills.status.broken",
  unresolved: "skills.status.unresolved",
  filtered: "skills.status.filtered",
};

export type SkillTone = "ok" | "warn" | "error" | "muted";

export const AGENT_SKILL_STATUS_TONES: Record<AgentSkillStatus, SkillTone> = {
  ready: "ok",
  missing: "error",
  broken: "error",
  unresolved: "warn",
  filtered: "muted",
};

/** 状态含义的 i18n key（悬浮提示用，与 av 的 fail-closed 判据对齐）。 */
export const AGENT_SKILL_STATUS_HINTS: Record<AgentSkillStatus, string> = {
  ready: "skills.statusHint.ready",
  missing: "skills.statusHint.missing",
  broken: "skills.statusHint.broken",
  unresolved: "skills.statusHint.unresolved",
  filtered: "skills.statusHint.filtered",
};

export const SKILL_VERIFY_LABELS: Record<SkillVerifyStatus, string> = {
  ok: "skills.verify.ok",
  missing: "skills.verify.missing",
  mismatch: "skills.verify.mismatch",
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
export function sourceLabel(
  t: SkillTranslate,
  source: string,
  sourceType: string,
  revision?: string | null,
): string {
  if (sourceType === "local") return t("skills.source.local", { path: source });
  return revision ? `${source}@${revision.slice(0, 12)}` : source;
}

/** 色调 → 标签类名（`.tag.ok` / `.tag.warn` / `.tag.error` 均在 styles/components/controls.css）。 */
export function toneClass(tone: SkillTone): string {
  if (tone === "ok") return "tag ok";
  if (tone === "warn") return "tag warn";
  if (tone === "error") return "tag error";
  return "tag";
}
