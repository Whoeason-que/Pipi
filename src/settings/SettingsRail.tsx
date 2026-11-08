// 设置右栏：当前分区说明、保存策略与本机数据位置。

import { useRightRail, RightRail } from "../app/right-rail";
import { IconGear } from "../icons";
import type { Settings } from "../types";
import { getSettingsSectionInfo, SettingsSectionIcon, type SettingsSection } from "./settings-sections";

export function SettingsRail({
  controller,
  section,
  settings,
  t,
  showLogout,
  onClose,
}: {
  controller: ReturnType<typeof useRightRail>;
  section: SettingsSection;
  settings: Settings;
  t: (key: string, params?: Record<string, string | number>) => string;
  showLogout?: boolean;
  onClose: () => void;
}) {
  const info = getSettingsSectionInfo(section, t);
  const defaultProvider = settings.providers.find((provider) => provider.id === settings.defaultProviderId);
  return <RightRail
    controller={controller}
    id="settings-rail"
    label={info.label}
    subtitle={t("settings.railSubtitle")}
    icon={<SettingsSectionIcon section={section} />}
    tabs={[
      {
        id: "section",
        label: t("settings.sectionTab"),
        content: <SectionInfo description={info.description} section={section} t={t} showLogout={showLogout} onClose={onClose} />,
      },
      {
        id: "storage",
        label: t("settings.storageTab"),
        content: <StorageInfo settings={settings} defaultProvider={defaultProvider?.id ?? null} t={t} />,
      },
    ]}
  />;
}

function SectionInfo({ description, section, t, showLogout, onClose }: {
  description: string;
  section: SettingsSection;
  t: (key: string, params?: Record<string, string | number>) => string;
  showLogout?: boolean;
  onClose: () => void;
}) {
  return <>
    <section className="right-rail-section">
      <p>{description}</p>
      <div className="right-rail-kv"><span>{t("settings.saveMode")}</span><span>{section === "remote" ? t("settings.manualApply") : t("settings.autoSave")}</span></div>
      <div className="right-rail-kv"><span>{t("settings.sectionLabel")}</span><span className="mono">{section}</span></div>
      {showLogout && <div className="right-rail-kv"><span>{t("settings.account")}</span><span>{t("settings.webRemoteAccess")}</span></div>}
    </section>
    <section className="right-rail-section">
      <h3>{t("settings.tips")}</h3>
      <p className="caption right-rail-note">
        {t("settings.autoSaveNote")} <kbd>Esc</kbd> {t("settings.escReturn")}
      </p>
      <div className="right-rail-actions-row">
        <button type="button" className="btn small ghost" onClick={onClose}>{t("common.back")}</button>
      </div>
    </section>
  </>;
}

function StorageInfo({ settings, defaultProvider, t }: {
  settings: Settings;
  defaultProvider: string | null;
  t: (key: string, params?: Record<string, string | number>) => string;
}) {
  return <>
    <section className="right-rail-section">
      <h3>{t("settings.file")}</h3>
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        <IconGear />
        <span className="truncate right-rail-item-main mono">~/.pipi/settings.json</span>
      </div>
      <p className="caption right-rail-note">{t("settings.fileNote")}</p>
    </section>
    <section className="right-rail-section">
      <h3>{t("settings.currentValues")}</h3>
      <div className="right-rail-kv"><span>{t("settings.theme")}</span><span className="mono">{settings.theme}</span></div>
      <div className="right-rail-kv"><span>{t("settings.providers")}</span><span>{t("settings.providerCount", { count: settings.providers.length })}</span></div>
      <div className="right-rail-kv"><span>{t("settings.defaultProvider")}</span><span className="mono">{defaultProvider ?? t("settings.notSet")}</span></div>
      <div className="right-rail-kv"><span>{t("settings.compaction")}</span><span>{settings.compaction.forkBeforeCompact ? t("settings.forkNewSession") : t("settings.inPlaceReplace")}</span></div>
      <div className="right-rail-kv"><span>{t("settings.retry")}</span><span>{t("settings.retrySummary", { attempts: settings.retry.maxAttempts, base: settings.retry.baseDelayMs, max: settings.retry.maxDelayMs })}</span></div>
    </section>
  </>;
}
