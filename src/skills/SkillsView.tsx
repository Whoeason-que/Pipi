// 技能库主区：store 概览（安装 / 全部校验）与技能卡片网格。
// 选中技能的版本、来源与启用情况放在右侧面板（见 SkillRail）。

import { useEffect, useState, type CSSProperties } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { useRightRail } from "../app/right-rail";
import { formatRuntimeError } from "../chat-runtime";
import ConfirmModal from "../components/ConfirmModal";
import { useI18n } from "../i18n";
import { IconCheck, IconDownload, IconPanelRight, IconRefresh, IconSkill } from "../icons";
import { invoke } from "../platform";
import type { SkillLibrary, SkillVerify, StoreSkill } from "../types";
import InstallSkillModal from "./InstallSkillModal";
import { SkillRail } from "./SkillRail";
import { shortHash, sourceLabel } from "./labels";

interface SkillsViewProps {
  library: SkillLibrary | null;
  /** 读取 store 失败时的可读原因（与侧栏同一份）。 */
  loadError: string | null;
  selected: string | null;
  onSelect: (name: string | null) => void;
  onRefresh: () => Promise<SkillLibrary | null>;
  onError: (message: string) => void;
}

export function SkillsView({
  library,
  loadError,
  selected,
  onSelect,
  onRefresh,
  onError,
}: SkillsViewProps) {
  const { t } = useI18n();
  const [working, setWorking] = useState<string | null>(null);
  const [verifyRows, setVerifyRows] = useState<Record<string, SkillVerify[]>>({});
  const [installOpen, setInstallOpen] = useState(false);
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const rail = useRightRail("skills");

  const skill = library?.skills.find((candidate) => candidate.name === selected) ?? null;
  const skillName = skill?.name ?? null;

  // 选中技能时展开右栏，清除选择时收起；用户手动关闭后不会反复弹开。
  useEffect(() => { rail.setOpen(Boolean(selected)); }, [selected]);

  const runVerify = async (name: string) => {
    setWorking(`verify:${name}`);
    try {
      const rows = await invoke("verify_store_skills", { names: [name] });
      setVerifyRows((previous) => ({ ...previous, [name]: rows }));
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const runVerifyAll = async () => {
    setWorking("verify:all");
    try {
      const rows = await invoke("verify_store_skills", {});
      const grouped: Record<string, SkillVerify[]> = {};
      for (const row of rows) {
        const list = grouped[row.name] ?? [];
        list.push(row);
        grouped[row.name] = list;
      }
      setVerifyRows(grouped);
      setNotice(
        rows.length === 0
          ? t("skills.storeEmptyTitle")
          : rows.every((row) => row.status === "ok")
            ? t("skills.noticeVerifiedAll", { count: rows.length })
            : t("skills.noticeVerifyIssues"),
      );
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const removeSkill = async (name: string) => {
    setWorking("remove");
    try {
      const removed = await invoke("remove_store_skill", { name });
      setNotice(t("skills.noticeRemoved", { name, count: removed }));
      onSelect(null);
      await onRefresh();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const chooseSkill = (name: string) => {
    onSelect(name);
    rail.setOpen(true);
  };

  const busy = working !== null;
  const totalVersions = library?.skills.reduce((total, item) => total + item.versions.length, 0) ?? 0;
  const missing = library?.skills.filter((item) => !currentVersion(item)?.present).length ?? 0;

  return (
    <div className="screen skills-screen" style={{ "--right-rail-width": `${rail.width}px` } as CSSProperties}>
      <div className="content-pane skills-main">
      <div className="screen-bar">
        <NavigationToggle />
        <span className="truncate caption crumb">
          {t("nav.skills")} · <b>{library?.storePath ?? "~/.av/skills"}</b>
        </span>
        <span className="spacer" />
        <button
          type="button"
          className={`icon-btn${rail.open ? " active" : ""}`}
          title={t("skills.detailsTitle")}
          aria-label={t("skills.toggleDetails")}
          aria-pressed={rail.open}
          onClick={rail.toggle}
        >
          <IconPanelRight />
        </button>
        <button
          type="button"
          className="btn ghost"
          onClick={() => void runVerifyAll()}
          disabled={busy || !library || library.skills.length === 0}
          title={t("skills.verifyAllHint")}
        >
          <IconCheck /> {t("skills.verifyAll")}
        </button>
        <button
          type="button"
          className="btn primary"
          onClick={() => setInstallOpen(true)}
          disabled={busy}
        >
          <IconDownload /> {t("skills.install")}
        </button>
      </div>

        <div className="skills-body">
          {notice && (
            <div className="skills-notice" role="status">
              <span>{notice}</span>
              <button type="button" className="error-dismiss" onClick={() => setNotice(null)} aria-label={t("common.close")}>
                ✕
              </button>
            </div>
          )}
          {!library ? (
            <div className="empty-state empty compact-empty skills-loading">
              <span className="icon-tile skills-hero-icon"><IconSkill /></span>
              <h2>{loadError ? t("skills.loadError") : t("skills.loading")}</h2>
              {loadError && <p>{loadError}</p>}
              <p>{t("skills.locationHint")}</p>
              <button type="button" className="btn ghost" onClick={() => void onRefresh()}>
                <IconRefresh /> {t("common.retry")}
              </button>
            </div>
          ) : (
            <>
              <header className="surface skills-hero skills-overview-hero">
                <span className="icon-tile skills-hero-icon"><IconSkill /></span>
                <div className="skills-hero-copy">
                  <h2>{t("skills.storeTitle")}</h2>
                  <p className="mono">{library.storePath}</p>
                  <p>{t("skills.storeDescription")}</p>
                </div>
              </header>

              <div className="skills-stat-grid overview">
                <div className="skills-stat">
                  <span>{t("skills.installed")}</span>
                  <strong>{library.skills.length}</strong>
                </div>
                <div className="skills-stat">
                  <span>{t("skills.contentVersions")}</span>
                  <strong>{totalVersions}</strong>
                </div>
                <div className={`skills-stat${missing ? " warn" : ""}`}>
                  <span>{t("skills.contentIssues")}</span>
                  <strong>{missing}</strong>
                </div>
                <div className="skills-stat">
                  <span>{t("skills.storage")}</span>
                  <strong className="mono">{t("skills.localFiles")}</strong>
                </div>
              </div>

              {library.skills.length === 0 ? (
                <div className="empty-state skills-empty">
                  <span className="icon-tile skills-hero-icon"><IconDownload /></span>
                  <h2>{t("skills.storeEmptyTitle")}</h2>
                  <p>{t("skills.storeEmptyHint")}</p>
                  <button type="button" className="btn primary" onClick={() => setInstallOpen(true)} disabled={busy}>
                    <IconDownload /> {t("skills.install")}
                  </button>
                </div>
              ) : (
                <section className="skills-panel">
                  <div className="skills-panel-head">
                    <h3 className="caption skills-sub">{t("skills.installed")} <span>{library.skills.length}</span></h3>
                    <span className="dim">
                      {skillName ? t("skills.viewingSkill", { name: skillName }) : t("skills.selectCardHint")}
                    </span>
                  </div>
                  <div className="skills-cards">
                    {library.skills.map((item) => (
                      <SkillCard key={item.name} skill={item} active={item.name === skillName}
                        onSelect={() => chooseSkill(item.name)} />
                    ))}
                  </div>
                </section>
              )}

              <details className="skills-notes">
                <summary>{t("skills.rulesTitle")}</summary>
                <p>
                  <b>{t("skills.rulesInstallTitle")}</b>{t("skills.rulesInstallBefore")}
                  <span className="mono"> [resources.skills].use </span>
                  {t("skills.rulesInstallAfter")}
                </p>
                <p>
                  <b>{t("skills.rulesReadTitle")}</b>{t("skills.rulesReadBefore")}
                  <span className="mono">agent.lock</span>{" "}
                  {t("skills.rulesReadAfter")}
                </p>
                <p>
                  {t("skills.rulesCli")}{" "}<span className="mono">av skill install | add | sync | verify | remove</span>
                </p>
              </details>
            </>
          )}
        </div>
      </div>

      <SkillRail
          controller={rail}
          skill={skill}
          verifyRows={skill ? verifyRows[skill.name] ?? [] : []}
          verifyBusy={working === `verify:${skillName}`}
          onVerify={() => { if (skill) void runVerify(skill.name); }}
          onRequestRemove={setRemoveTarget}
          onError={onError}
        />

      {installOpen && (
        <InstallSkillModal
          onClose={() => setInstallOpen(false)}
          onInstalled={async () => {
            await onRefresh();
          }}
          onError={onError}
        />
      )}
      {removeTarget && (
        <ConfirmModal
          title={t("skills.removeTitle")}
          message={t("skills.removeMessage", { name: removeTarget })}
          confirmLabel={t("skills.remove")}
          onConfirm={async () => {
            const name = removeTarget;
            setRemoveTarget(null);
            await removeSkill(name);
          }}
          onCancel={() => setRemoveTarget(null)}
        />
      )}
    </div>
  );
}

function currentVersion(skill: StoreSkill) {
  return skill.versions.find((version) => version.contentHash === skill.currentHash);
}

function SkillCard({ skill, active, onSelect }: { skill: StoreSkill; active: boolean; onSelect: () => void }) {
  const { t } = useI18n();
  const current = currentVersion(skill);
  const healthy = !!current?.present;
  return (
    <button type="button" className={`surface surface-interactive skills-card${active ? " active" : ""}`} aria-pressed={active} onClick={onSelect}>
      <div className="skills-card-head">
        <span className="icon-tile skills-card-icon"><IconSkill /></span>
        <span className="truncate skills-card-name mono">{skill.name}</span>
        <span className={`skill-dot ${healthy ? "ok" : "error"}`} title={healthy ? t("skills.currentVersionPresent") : t("skills.currentVersionMissing")} />
      </div>
      <p className="skills-card-desc">{skill.description ?? t("skills.noDescription")}</p>
      <div className="caption skills-card-foot">
        <span className="mono" title={skill.source}>{sourceLabel(t, skill.source, skill.sourceType)}</span>
        <span>{t("skills.versionCount", { count: skill.versions.length })}</span>
        <span className="mono">{shortHash(skill.currentHash)}</span>
      </div>
    </button>
  );
}
