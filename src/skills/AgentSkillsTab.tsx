// Agent 工作台「技能」页：这个 Agent 声明启用的 store 技能（use + agent.lock 状态）、
// 可启用清单，以及同步 / 修复。写操作立即落盘（与 av CLI 共用实现），随后重建
// 临时测试会话，让改动当场生效并暴露 fail-closed 错误。

import { useCallback, useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { useI18n } from "../i18n";
import { IconRefresh, IconSkill } from "../icons";
import { invoke } from "../platform";
import type { AgentSkills, SkillSyncReport } from "../types";
import {
  AGENT_SKILL_STATUS_HINTS,
  AGENT_SKILL_STATUS_LABELS,
  AGENT_SKILL_STATUS_TONES,
  shortHash,
  toneClass,
  type SkillTranslate,
} from "./labels";

interface AgentSkillsTabProps {
  agentName: string;
  onError: (message: string) => void;
  /** 改动落盘后：重建临时测试会话，让改动立刻生效。 */
  onChanged: () => Promise<void> | void;
}

function reportLines(t: SkillTranslate, report: SkillSyncReport): string[] {
  const lines: string[] = [];
  if (report.resolved.length) lines.push(t("skills.syncReportResolved", { names: report.resolved.join(", ") }));
  if (report.installed.length) lines.push(t("skills.syncReportInstalled", { names: report.installed.join(", ") }));
  if (report.repaired.length) lines.push(t("skills.syncReportRepaired", { names: report.repaired.join(", ") }));
  if (report.ready.length) lines.push(t("skills.syncReportReady", { names: report.ready.join(", ") }));
  return [...lines, ...report.log];
}

export function AgentSkillsTab({ agentName, onError, onChanged }: AgentSkillsTabProps) {
  const { t } = useI18n();
  const [view, setView] = useState<AgentSkills | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const [repair, setRepair] = useState(false);
  const [syncLog, setSyncLog] = useState<string[] | null>(null);

  useEffect(() => {
    let active = true;
    setView(null);
    setLoadError(null);
    setSyncLog(null);
    void invoke("agent_skills", { agentName })
      .then((next) => {
        if (active) setView(next);
      })
      .catch((errorValue) => {
        if (!active) return;
        const message = formatRuntimeError(errorValue);
        setLoadError(message);
        onError(message);
      });
    return () => {
      active = false;
    };
  }, [agentName, onError, reloadToken]);

  const reload = useCallback(async () => {
    setReloadToken((token) => token + 1);
    await onChanged();
  }, [onChanged]);

  const toggle = async (name: string, enabled: boolean) => {
    setWorking(`toggle:${name}`);
    try {
      await invoke("set_skill_enabled", { agentName, name, enabled });
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
      await reload();
    }
  };

  const sync = async () => {
    setWorking("sync");
    try {
      const report = await invoke("sync_agent_skills", { agentName, repair });
      setSyncLog(reportLines(t, report));
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
      await reload();
    }
  };

  if (!view) {
    return (
      <section className="settings-section">
        {loadError ? (
          <>
            <p className="field-error">{t("skills.loadDeclarationsFailed", { error: loadError })}</p>
            <button
              type="button"
              className="btn ghost"
              onClick={() => setReloadToken((token) => token + 1)}
            >
              <IconRefresh /> {t("common.retry")}
            </button>
          </>
        ) : (
          <p className="dim">{t("skills.loadingDeclarations")}</p>
        )}
      </section>
    );
  }

  const busy = working !== null;
  const declaredByLabel =
    view.declaredBy === "project-local"
      ? t("skills.declaredByProjectLocal")
      : view.declaredBy === "project"
        ? t("skills.declaredByProject")
        : t("skills.declaredByAgent");

  return (
    <section className="settings-section agent-skills">
      <div className="agent-skills-overview">
        <span className="icon-tile agent-skills-overview-icon"><IconSkill /></span>
        <div>
          <strong>{view.declared.length > 0 ? t("skills.enabledCount", { count: view.declared.length }) : t("skills.noneEnabled")}</strong>
          <p>{t("skills.overviewDescription")}</p>
        </div>
        <span className={`tag ${view.declared.length > 0 ? "ok" : ""}`}>{view.declared.length} / {view.declared.length + view.available.length}</span>
      </div>
      <div className="settings-summary-row">
        <span className="field-label">{t("skills.declaration")}</span>
        {view.declared.length > 0 ? (
          <span>
            {declaredByLabel}
            {view.declaresInProject && <span className="tag warn" title={t("skills.projectLayerHint")}>{t("skills.projectLayer")}</span>}
          </span>
        ) : view.declaredBy ? (
          <span>{t("skills.explicitlyDisabled", { source: declaredByLabel })}</span>
        ) : (
          <span className="dim">{t("skills.noUseDeclared")}</span>
        )}
      </div>
      <div className="settings-summary-row">
        <span className="field-label">{t("skills.contract")}</span>
        <span className="mono break-all" title={view.contractPath}>
          {view.contractPath}
        </span>
      </div>
      <div className="settings-summary-row">
        <span className="field-label">{t("skills.lock")}</span>
        <span className="mono break-all" title={view.lockPath}>
          {view.lockPath}
        </span>
      </div>

      {view.lockError && (
        <p className="field-error" role="alert">
          {view.lockError}
        </p>
      )}

      <div className="settings-field">
        <span className="field-label">{t("skills.enabledHeading", { count: view.declared.length })}</span>
        {view.declared.length === 0 && <span className="dim">{t("skills.noEnabledSkills")}</span>}
        <div className="agent-skills-list">
          {view.declared.map((skill) => (
            <div className="agent-skills-row" key={skill.name}>
              <div className="info">
                <div className="name">
                  <span className="mono">{skill.name}</span>
                  <span className={toneClass(AGENT_SKILL_STATUS_TONES[skill.status])} title={skill.filter ?? t(AGENT_SKILL_STATUS_HINTS[skill.status])}>
                    {t(AGENT_SKILL_STATUS_LABELS[skill.status])}
                  </span>
                </div>
                <div className="dim ellipsis" title={skill.description ?? undefined}>
                  {skill.description ?? (skill.contentHash ? shortHash(skill.contentHash) : "—")}
                </div>
              </div>
              <button
                type="button"
                className="btn small ghost"
                disabled={busy}
                onClick={() => void toggle(skill.name, false)}
              >
                {working === `toggle:${skill.name}` ? "…" : t("skills.disable")}
              </button>
            </div>
          ))}
        </div>
      </div>

      <div className="settings-field">
        <span className="field-label">{t("skills.availableHeading", { count: view.available.length })}</span>
        {view.available.length === 0 && (
          <span className="dim">
            {t("skills.noAvailable")}
          </span>
        )}
        <div className="agent-skills-list">
          {view.available.map((skill) => (
            <div className="agent-skills-row" key={skill.name}>
              <div className="info">
                <div className="name">
                  <span className="mono">{skill.name}</span>
                  {!skill.present && <span className="tag warn">{t("skills.contentMissing")}</span>}
                </div>
                <div className="dim ellipsis" title={skill.description ?? undefined}>
                  {skill.description ?? shortHash(skill.contentHash)}
                </div>
              </div>
              <button
                type="button"
                className="btn small"
                disabled={busy || !skill.present}
                title={skill.present ? t("skills.enableHint") : t("skills.enableMissingHint")}
                onClick={() => void toggle(skill.name, true)}
              >
                {working === `toggle:${skill.name}` ? "…" : t("skills.enable")}
              </button>
            </div>
          ))}
        </div>
      </div>

      <div className="settings-field">
        <span className="field-label">{t("skills.repair")}</span>
        <div className="agent-skills-sync">
          <button type="button" className="btn ghost" disabled={busy || view.declared.length === 0} onClick={() => void sync()}>
            <IconRefresh /> {working === "sync" ? t("skills.syncing") : t("skills.syncDeclared")}
          </button>
          <label className="ios-toggle">
            <input type="checkbox" checked={repair} disabled={busy} onChange={(event) => setRepair(event.target.checked)} />
            <span className="slider" />
            <span className="toggle-label">{t("skills.repairDrift")}</span>
          </label>
        </div>
        <span className="hint">
          {t("skills.syncHint")}
        </span>
        {syncLog && syncLog.length > 0 && <pre className="skill-log">{syncLog.join("\n")}</pre>}
      </div>

      {view.only || view.exclude || view.sources ? (
        <div className="settings-field">
          <span className="field-label">{t("skills.filtersHeading")}</span>
          <div className="settings-tags">
            {view.only && <span className="tag" title={t("skills.onlyHint")}>only: {view.only.join(", ")}</span>}
            {view.exclude && <span className="tag" title={t("skills.excludeHint")}>exclude: {view.exclude.join(", ")}</span>}
            {view.sources && <span className="tag" title={t("skills.sourcesHint")}>sources: {view.sources.join(", ")}</span>}
          </div>
        </div>
      ) : null}

      <p className="hint">
        {t("skills.toggleHint")}
      </p>
    </section>
  );
}
