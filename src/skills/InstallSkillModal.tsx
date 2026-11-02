// 安装技能到全局 store（~/.av/skills）的弹层：来源 + 选装策略 + dry-run 预览。
// 只装不启用 —— 启用由 Agent 工作台的「技能」页完成（写 [resources.skills].use）。

import { useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { useI18n } from "../i18n";
import { IconClose } from "../icons";
import { invoke } from "../platform";
import type { SkillInstallReport } from "../types";
import { shortHash } from "./labels";

type SelectionMode = "auto" | "all" | "only";

/** 选装策略的 i18n key（数据值仍是 auto / all / only）。 */
const MODE_LABELS: Record<SelectionMode, string> = {
  auto: "skills.modeAuto",
  all: "skills.modeAll",
  only: "skills.modeOnly",
};

export default function InstallSkillModal({
  onClose,
  onInstalled,
  onError,
}: {
  onClose: () => void;
  onInstalled: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const { t } = useI18n();
  const [source, setSource] = useState("");
  const [mode, setMode] = useState<SelectionMode>("auto");
  const [names, setNames] = useState("");
  const [dryRun, setDryRun] = useState(false);
  const [working, setWorking] = useState(false);
  const [report, setReport] = useState<SkillInstallReport | null>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const submit = async () => {
    const trimmed = source.trim();
    if (!trimmed) {
      onError(t("skills.sourceRequired"));
      return;
    }
    const selected =
      mode === "only"
        ? names.split(/[\s,，]+/).map((name) => name.trim()).filter(Boolean)
        : null;
    if (mode === "only" && (!selected || selected.length === 0)) {
      onError(t("skills.namesRequired"));
      return;
    }
    setWorking(true);
    setReport(null);
    try {
      const result = await invoke("install_skill", {
        source: trimmed,
        skills: selected,
        all: mode === "all",
        dryRun,
      });
      setReport(result);
      if (!dryRun) await onInstalled();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(false);
    }
  };

  const finished = Boolean(report && !report.dryRun);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal skill-install-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="skill-install-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="skill-install-title">{t("skills.install")}</h2>
          <button type="button" className="icon-btn close-btn" onClick={onClose} title={t("common.close")}>
            <IconClose />
          </button>
        </div>
        <div className="modal-section">
          <div className="settings-field">
            <label className="field-label" htmlFor="skill-install-source">
              {t("skills.sourceLabel")}
            </label>
            <input
              id="skill-install-source"
              className="mono"
              value={source}
              onChange={(event) => setSource(event.target.value)}
              placeholder={t("skills.sourcePlaceholder")}
              spellCheck={false}
              autoFocus
            />
            <span className="hint">
              {t("skills.sourceHint")}
            </span>
          </div>
          <div className="settings-field">
            <span className="field-label">{t("skills.selectionMode")}</span>
            <div className="tool-row vertical-choices">
              {(Object.keys(MODE_LABELS) as SelectionMode[]).map((candidate) => (
                <label key={candidate} className="tool-check">
                  <input
                    type="radio"
                    name="skill-install-mode"
                    checked={mode === candidate}
                    onChange={() => setMode(candidate)}
                  />
                  <span>{t(MODE_LABELS[candidate])}</span>
                </label>
              ))}
            </div>
          </div>
          {mode === "only" && (
            <div className="settings-field">
              <label className="field-label" htmlFor="skill-install-names">
                {t("skills.skillNames")}
              </label>
              <input
                id="skill-install-names"
                className="mono"
                value={names}
                onChange={(event) => setNames(event.target.value)}
                placeholder="pdf, writer"
                spellCheck={false}
              />
              <span className="hint">{t("skills.namesHint")}</span>
            </div>
          )}
          <label className="ios-toggle">
            <input
              type="checkbox"
              checked={dryRun}
              onChange={(event) => setDryRun(event.target.checked)}
            />
            <span className="slider" />
            <span className="toggle-label">{t("skills.dryRun")}</span>
          </label>

          {report && (
            <div className="skill-report">
              {report.installed.map((item) => (
                <div className="skill-report-row" key={item.name}>
                  <span className={`tag${item.unchanged ? "" : " ok"}`}>
                    {item.unchanged ? t("skills.upToDate") : report.dryRun ? t("skills.willInstall") : t("skills.installedDone")}
                  </span>
                  <span className="mono">{item.name}</span>
                  <span className="dim">{shortHash(item.contentHash)}</span>
                </div>
              ))}
              {report.skipped.map((item) => (
                <div className="skill-report-row" key={item.path}>
                  <span className="tag warn">{t("skills.skipped")}</span>
                  <span className="mono">{item.path}</span>
                  <span className="dim">{item.reason}</span>
                </div>
              ))}
              {report.log.length > 0 && <pre className="skill-log">{report.log.join("\n")}</pre>}
            </div>
          )}

          <p className="hint">
            {t("skills.installNotEnableHint")}
          </p>
        </div>
        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onClose} disabled={working}>
            {finished ? t("skills.done") : t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => void submit()}
            disabled={working}
          >
            {working ? t("skills.working") : dryRun ? t("skills.preview") : t("skills.installAction")}
          </button>
        </div>
      </div>
    </div>
  );
}
