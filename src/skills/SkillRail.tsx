// 技能右栏：选中技能的版本、来源、校验与 Agent 启用情况。
// 主区保留技能网格，右栏只承担辅助阅读与操作。

import { useCallback, useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { RightRailEmpty } from "../components/RightRailEmpty";
import { useI18n } from "../i18n";
import { IconRefresh, IconSkill } from "../icons";
import { invoke } from "../platform";
import type { SkillUsage, SkillVerify, StoreSkill } from "../types";
import { RightRail, type RightRailController } from "../app/right-rail";
import {
  AGENT_SKILL_STATUS_HINTS,
  AGENT_SKILL_STATUS_LABELS,
  AGENT_SKILL_STATUS_TONES,
  SKILL_VERIFY_LABELS,
  SKILL_VERIFY_TONES,
  shortHash,
  sourceLabel,
  toneClass,
} from "./labels";

export function SkillRail({
  controller,
  skill,
  verifyRows,
  verifyBusy,
  onVerify,
  onRequestRemove,
  onError,
}: {
  controller: RightRailController;
  skill: StoreSkill | null;
  verifyRows: SkillVerify[];
  verifyBusy: boolean;
  onVerify: () => void;
  onRequestRemove: (name: string) => void;
  onError: (message: string) => void;
}) {
  const { t } = useI18n();
  const [tab, setTab] = useState("overview");
  const [usage, setUsage] = useState<SkillUsage[] | null>(null);
  const [usageError, setUsageError] = useState<string | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const name = skill?.name ?? null;

  const loadUsage = useCallback(async (skillName: string) => {
    try {
      setUsage(await invoke("skill_usage", { name: skillName }));
      setUsageError(null);
    } catch (errorValue) {
      setUsage(null);
      setUsageError(formatRuntimeError(errorValue));
    }
  }, []);

  useEffect(() => {
    setTab("overview");
    if (!name) {
      setUsage(null);
      setUsageError(null);
      return;
    }
    void loadUsage(name);
  }, [name, loadUsage]);

  const toggleAgent = async (agentName: string, enabled: boolean) => {
    if (!name) return;
    setWorking(`toggle:${agentName}`);
    try {
      await invoke("set_skill_enabled", { agentName, name, enabled });
      await loadUsage(name);
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const current = skill?.versions.find((version) => version.contentHash === skill.currentHash);
  const enabledCount = usage?.filter((row) => row.enabled).length ?? 0;
  const busy = working !== null;

  return <RightRail
    controller={controller}
    id="skill-rail"
    label={skill?.name ?? t("skills.detailsTitle")}
    subtitle={skill ? sourceLabel(t, skill.source, skill.sourceType) : t("skills.selectSkillHint")}
    icon={<IconSkill />}
    actions={skill ? <>
      <button type="button" className="icon-btn" title={t("skills.verifyContent")} aria-label={t("skills.verifyContent")} disabled={verifyBusy} onClick={onVerify}>
        <IconRefresh />
      </button>
      <button type="button" className="btn small" disabled={verifyBusy} onClick={() => onRequestRemove(skill.name)}>
        {t("skills.remove")}
      </button>
    </> : undefined}
    tabs={skill ? [
      {
        id: "overview",
        label: t("skills.tabOverview"),
        badge: skill.versions.length,
        content: <Overview skill={skill} current={current} verifyRows={verifyRows} />,
      },
      {
        id: "usage",
        label: t("skills.tabUsage"),
        badge: usage ? enabledCount : undefined,
        content: <Usage usage={usage} usageError={usageError} busy={busy} working={working}
          onToggle={toggleAgent} />,
      },
    ] : undefined}
    activeTab={tab}
    onTabChange={setTab}
  >
    {!skill && <RightRailEmpty
      icon={<IconSkill />}
      title={t("skills.noSelection")}
      description={t("skills.noSelectionDescription")}
    />}
  </RightRail>;
}

function Overview({ skill, current, verifyRows }: {
  skill: StoreSkill;
  current: StoreSkill["versions"][number] | undefined;
  verifyRows: SkillVerify[];
}) {
  const { t } = useI18n();
  return <>
    <section className="right-rail-section">
      <p>{skill.description ?? t("skills.noDescription")}</p>
      <div className="right-rail-kv"><span>{t("skills.currentVersion")}</span><span className="mono" title={skill.currentHash}>{shortHash(skill.currentHash)}</span></div>
      <div className="right-rail-kv"><span>{t("skills.sourcePath")}</span><span className="mono">{skill.skillPath ?? "—"}</span></div>
      <div className="right-rail-kv"><span>{t("skills.installedAt")}</span><span>{formatInstalledAt(skill.installedAt)}</span></div>
      <div className="right-rail-kv"><span>{t("skills.content")}</span><span className={current?.present ? "tag ok" : "tag error"}>{current?.present ? t("skills.present") : t("skills.missing")}</span></div>
    </section>
    <section className="right-rail-section">
      <h3>{t("skills.versionsHeading", { count: skill.versions.length })}</h3>
      <div className="right-rail-list">
        {skill.versions.map((version) => {
          const rows = verifyRows.filter((row) => row.contentHash === version.contentHash);
          const active = version.contentHash === skill.currentHash;
          return <div className={`right-rail-item${active ? " active" : ""}`} key={version.contentHash}>
            <span className="mono truncate right-rail-item-main" title={version.contentHash}>{shortHash(version.contentHash)}</span>
            {active && <small>{t("skills.current")}</small>}
            {version.present ? <span className="tag ok">{t("skills.present")}</span> : <span className="tag error">{t("skills.missing")}</span>}
            {rows.map((row) => <span key={`${row.status}:${row.actualHash ?? ""}`}
              className={toneClass(SKILL_VERIFY_TONES[row.status])}
              title={row.actualHash ? t("skills.actualHash", { hash: row.actualHash }) : undefined}>{t(SKILL_VERIFY_LABELS[row.status])}</span>)}
          </div>;
        })}
      </div>
      <p className="caption right-rail-note">{t("skills.verifyNote")}</p>
    </section>
  </>;
}

function Usage({ usage, usageError, busy, working, onToggle }: {
  usage: SkillUsage[] | null;
  usageError: string | null;
  busy: boolean;
  working: string | null;
  onToggle: (agentName: string, enabled: boolean) => void;
}) {
  const { t } = useI18n();
  if (usageError) return <p className="field-error">{usageError}</p>;
  if (!usage) return <p className="caption right-rail-note">{t("skills.loadingUsage")}</p>;
  if (usage.length === 0) return <RightRailEmpty title={t("skills.noAgents")} description={t("skills.noAgentsDescription")} />;
  return <>
    <section className="right-rail-section">
      <h3>{t("skills.agentsHeading", { count: usage.filter((row) => row.enabled).length })}</h3>
      <div className="right-rail-list">
        {usage.map((row) => <div className="right-rail-item" key={row.agentName}>
          <span className="right-rail-avatar">{row.agentName.slice(0, 1).toUpperCase()}</span>
          <span className="truncate right-rail-item-main mono">{row.agentName}</span>
          {row.enabled && row.status ? <span className={toneClass(AGENT_SKILL_STATUS_TONES[row.status])}
            title={row.filter ?? t(AGENT_SKILL_STATUS_HINTS[row.status])}>{t(AGENT_SKILL_STATUS_LABELS[row.status])}</span>
            : <small>{t("skills.notEnabled")}</small>}
          <button type="button" className={row.enabled ? "btn small ghost" : "btn small"} disabled={busy}
            onClick={() => onToggle(row.agentName, !row.enabled)}>
            {working === `toggle:${row.agentName}` ? "…" : row.enabled ? t("skills.disable") : t("skills.enable")}
          </button>
        </div>)}
      </div>
    </section>
    <p className="caption right-rail-note">{t("skills.enableNote")}</p>
  </>;
}

function formatInstalledAt(value: string | null): string {
  if (!value) return "—";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? new Date(timestamp).toLocaleString() : value;
}
