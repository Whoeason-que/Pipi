import { formatRetryDelay } from "../chat-runtime";
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
    "providers",
  ];
  if (showLogout) sections.push("account");

  const sectionMeta = (id: SettingsSection): string => {
    switch (id) {
      case "appearance":
        return `${resolveTheme(settings.theme).label} · ${locale.toUpperCase()}`;
      case "context":
        return settings.compaction.forkBeforeCompact
          ? "压缩前分叉"
          : "原地压缩";
      case "retry":
        return `${settings.retry.maxAttempts} 次尝试 · ${formatRetryDelay(settings.retry.baseDelayMs)}起`;
      case "providers": {
        const fallback = settings.providers.find(
          (item) => item.id === settings.defaultProviderId,
        );
        return `${settings.providers.length} 家 · 默认 ${fallback?.name ?? "未设置"}`;
      }
      case "account":
        return "Web 访问保护";
    }
  };

  return (
    <nav
      className="settings-nav"
      role="tablist"
      aria-label={t("settings.title")}
      aria-orientation="vertical"
    >
      <span className="settings-nav-title">{t("settings.title")}</span>
      {sections.map((id) => (
        <button
          key={id}
          type="button"
          role="tab"
          id={`settings-tab-${id}`}
          aria-controls="settings-content"
          aria-selected={section === id}
          className={`settings-nav-item${section === id ? " active" : ""}`}
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
