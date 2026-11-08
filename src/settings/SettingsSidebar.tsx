import { formatRetryDelay } from "../chat-runtime";
import { isTauriRuntime } from "../platform";
import { useI18n } from "../i18n";
import { resolveTheme } from "../theme";
import type { Settings } from "../types";
import {
  getSettingsSectionInfo,
  SettingsSectionIcon,
  type SettingsSection,
} from "./settings-sections";

export default function SettingsSidebar({
  settings,
  section,
  showLogout,
  onSelect,
}: {
  settings: Settings;
  section: SettingsSection;
  showLogout: boolean;
  onSelect: (section: SettingsSection) => void;
}) {
  const { locale, t } = useI18n();
  const sections: SettingsSection[] = [
    "appearance",
    "context",
    "retry",
    "notifications",
    "providers",
  ];
  if (isTauriRuntime()) sections.push("remote");
  if (showLogout) sections.push("account");

  const sectionMeta = (id: SettingsSection): string => {
    switch (id) {
      case "appearance":
        return `${t(`themes.${resolveTheme(settings.theme).id}.label`)} · ${locale.toUpperCase()}`;
      case "context":
        return settings.compaction.forkBeforeCompact
          ? t("settings.forkBeforeCompactShort")
          : t("settings.inPlaceShort");
      case "retry":
        return t("settings.retryMeta", {
          attempts: settings.retry.maxAttempts,
          base: formatRetryDelay(settings.retry.baseDelayMs),
        });
      case "notifications":
        return t("notifications.thisDevice");
      case "providers": {
        const fallback = settings.providers.find(
          (item) => item.id === settings.defaultProviderId,
        );
        return t("settings.providersMeta", {
          count: settings.providers.length,
          name: fallback?.name ?? t("settings.notSet"),
        });
      }
      case "remote":
        return t("remoteAccess.navHint");
      case "account":
        return t("settings.webAccessProtection");
    }
  };

  return (
    <nav
      className="sidebar-list settings-nav"
      role="tablist"
      aria-label={t("settings.title")}
      aria-orientation="vertical"
    >
      <span className="caption sb-sec settings-nav-title">{t("settings.title")}</span>
      {sections.map((id) => (
        <button
          key={id}
          type="button"
          role="tab"
          id={`settings-tab-${id}`}
          aria-controls="settings-content"
          aria-selected={section === id}
          className={`sidebar-item settings-nav-item${section === id ? " active" : ""}`}
          onClick={() => onSelect(id)}
        >
          <span className="settings-nav-icon">
            <SettingsSectionIcon section={id} />
          </span>
          <span className="settings-nav-copy">
            <span className="name">{getSettingsSectionInfo(id, t).label}</span>
            <span className="meta">{sectionMeta(id)}</span>
          </span>
        </button>
      ))}
    </nav>
  );
}
