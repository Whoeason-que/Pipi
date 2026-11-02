import { useContext, useEffect, useRef, useState, type CSSProperties } from "react";
import { errorMessage } from "../errors.ts";
import { LayoutContext } from "../app/layout-context";
import { NavigationToggle } from "../app/AppNavigation";
import { useRightRail } from "../app/right-rail";
import { catalogSourceLabel, type CatalogProvider } from "../catalog";
import { loadCatalog, resetCatalog } from "../catalog-client";
import { IconBack, IconLayers, IconPanelRight, IconPlus, IconRefresh } from "../icons";
import IconThemeSettings from "./IconThemeSettings";
import RemoteAccessPanel from "./RemoteAccessPanel";
import NotificationsPanel from "./NotificationsPanel";
import { SettingsRail } from "./SettingsRail";
import { isTauriRuntime, logout } from "../platform";
import {
  API_LABELS,
  API_MODE_LABELS,
  type ProviderConfig,
  type Settings,
} from "../types";
import { useI18n } from "../i18n";

import { PresetPicker, ProviderForm } from "./ProviderEditor";
import { LocaleOption, ThemeOption } from "./AppearanceOptions";
import { BUILTIN_THEMES, resolveTheme } from "../theme";
import {
  getSettingsSectionInfo,
  SettingsSectionIcon,
  type SettingsSection,
} from "./settings-sections";
interface SettingsViewProps {
  section: SettingsSection;
  settings: Settings;
  onChange: (next: Settings) => void | Promise<void>;
  onClose: () => void;
  showLogout?: boolean;
}

export default function SettingsView({
  settings,
  section,
  onChange,
  onClose,
  showLogout,
}: SettingsViewProps) {
  const { locale, setLocale, t } = useI18n();
  const { navigationOpen } = useContext(LayoutContext);
  const rail = useRightRail("settings");
  const sectionInfo = getSettingsSectionInfo(section, t);
  const [editing, setEditing] = useState<ProviderConfig | "new" | null>(null);
  const [preset, setPreset] = useState<CatalogProvider | null>(null);
  const [picking, setPicking] = useState(false);
  const [catalogNote, setCatalogNote] = useState<string | null>(null);
  const [refreshingCatalog, setRefreshingCatalog] = useState(false);
  const [draft, setDraft] = useState(settings);
  const draftRef = useRef(settings);

  /** 手动刷新模型目录（默认 24h 才自动刷新，这里给用户一个立即刷新的出口）。 */
  const refreshCatalog = async () => {
    if (refreshingCatalog) return;
    setRefreshingCatalog(true);
    setCatalogNote(null);
    try {
      resetCatalog();
      const next = await loadCatalog(true);
      setCatalogNote(
        t("settings.catalogRefreshed", {
          count: next.providers.length,
          source: catalogSourceLabel(next),
        }),
      );
    } catch (reason) {
      setCatalogNote(
        t("settings.catalogRefreshFailed", { error: errorMessage(reason) }),
      );
    } finally {
      setRefreshingCatalog(false);
    }
  };

  const closeEditor = () => {
    setEditing(null);
    setPreset(null);
    setPicking(false);
  };

  useEffect(() => {
    draftRef.current = settings;
    setDraft(settings);
  }, [settings]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.key === "Escape" &&
        !(navigationOpen && window.matchMedia("(max-width: 860px)").matches)
      )
        onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, navigationOpen]);

  const commit = (update: (previous: Settings) => Settings) => {
    const next = update(draftRef.current);
    draftRef.current = next;
    setDraft(next);
    void onChange(next);
  };

  const saveProvider = (provider: ProviderConfig) => {
    commit((previous) => {
      const providers = [...previous.providers];
      const index = providers.findIndex((item) => item.id === provider.id);
      if (index >= 0) providers[index] = provider;
      else providers.push(provider);
      return { ...previous, providers };
    });
    closeEditor();
  };

  const deleteProvider = (id: string) => {
    if (
      !confirm(
        t("settings.deleteProviderConfirm", { id }),
      )
    )
      return;
    commit((previous) => ({
      ...previous,
      providers: previous.providers.filter((provider) => provider.id !== id),
      defaultProviderId:
        previous.defaultProviderId === id ? null : previous.defaultProviderId,
    }));
  };

  return (
    <div className="screen settings-view" style={{ "--right-rail-width": `${rail.width}px` } as CSSProperties}>
      <div className="content-pane settings-shell">
      <div className="screen-bar">
        <NavigationToggle />
        <button
          type="button"
          className="icon-btn"
          title={t("common.back")}
          aria-label={t("common.back")}
          onClick={onClose}
        >
          <IconBack />
        </button>
        <span className="truncate caption crumb">
          {t("settings.title")} · <b>{sectionInfo.label}</b>
        </span>
        <span className="spacer" />
        <button
          type="button"
          className={`icon-btn${rail.open ? " active" : ""}`}
          title={t("settings.infoPanel")}
          aria-label={t("settings.toggleInfoPanel")}
          aria-pressed={rail.open}
          onClick={rail.toggle}
        >
          <IconPanelRight />
        </button>
        {section !== "remote" && <span className="caption settings-autosave">
          <span aria-hidden="true" />
          {t("common.saveAuto")}
        </span>}
      </div>

      <div
        className="settings-main"
        role="tabpanel"
        id="settings-content"
        aria-labelledby={`settings-tab-${section}`}
      >
        <div className="settings-main-head">
          <span className="icon-tile settings-section-icon">
            <SettingsSectionIcon section={section} />
          </span>
          <div>
            <h2>{sectionInfo.label}</h2>
            <p>{sectionInfo.description}</p>
          </div>
        </div>

        <div className="settings-main-body">
          {section === "notifications" && <NotificationsPanel settings={draft.notifications} onChange={notifications => commit(previous => ({ ...previous, notifications }))} />}
          {section === "appearance" && (
            <>
              <section className="surface settings-block">
                <span className="label">{t("settings.language")}</span>
                <div className="locale-row">
                  <LocaleOption
                    active={locale === "zh"}
                    code="ZH"
                    label={t("settings.zh")}
                    onClick={() => setLocale("zh")}
                  />
                  <LocaleOption
                    active={locale === "en"}
                    code="EN"
                    label={t("settings.en")}
                    onClick={() => setLocale("en")}
                  />
                </div>
                <span className="hint settings-block-note">
                  {t("settings.languageDesc")}
                </span>
              </section>

              <IconThemeSettings />
              <section className="surface settings-block">
                <span className="label">{t("settings.theme")}</span>
                <div className="theme-grid">
                  {BUILTIN_THEMES.map((item) => {
                    const activeTheme = resolveTheme(draft.theme);
                    return (
                      <ThemeOption
                        key={item.id}
                        active={activeTheme.id === item.id}
                        theme={item}
                        onClick={() =>
                          commit((previous) => ({ ...previous, theme: item.id }))
                        }
                      />
                    );
                  })}
                </div>
              </section>
            </>
          )}

          {section === "context" && (
            <section className="surface settings-block">
              <span className="label">{t("settings.compaction")}</span>
              <div className="setting-checks">
                <label className="ios-toggle settings-toggle">
                  <input
                    type="checkbox"
                    checked={draft.compaction.forkBeforeCompact}
                    onChange={() =>
                      commit((previous) => ({
                        ...previous,
                        compaction: {
                          ...previous.compaction,
                          forkBeforeCompact:
                            !previous.compaction.forkBeforeCompact,
                        },
                      }))
                    }
                  />
                  <span className="slider" />
                  <span className="toggle-label">
                    <strong>{t("settings.forkBeforeCompact")}</strong>
                    <span>{t("settings.forkBeforeCompactDesc")}</span>
                  </span>
                </label>
                <label className="ios-toggle settings-toggle">
                  <input
                    type="checkbox"
                    checked={draft.compaction.archiveOriginal}
                    disabled={!draft.compaction.forkBeforeCompact}
                    onChange={() =>
                      commit((previous) => ({
                        ...previous,
                        compaction: {
                          ...previous.compaction,
                          archiveOriginal: !previous.compaction.archiveOriginal,
                        },
                      }))
                    }
                  />
                  <span className="slider" />
                  <span className="toggle-label">
                    <strong>{t("settings.archiveOriginal")}</strong>
                    <span>{t("settings.archiveOriginalDesc")}</span>
                  </span>
                </label>
                <div className="settings-info-note">
                  <IconLayers />
                  <span>
                    {t("settings.inPlaceNote")}
                  </span>
                </div>
              </div>
            </section>
          )}

          {section === "retry" && (
            <section className="surface settings-block retry-settings">
              <span className="label">{t("settings.retry")}</span>
              <div className="settings-field compact-threshold-field">
                <label htmlFor="retry-max-attempts">{t("settings.maxAttempts")}</label>
                <div className="inline-value">
                  <input
                    id="retry-max-attempts"
                    className="text-input mono"
                    type="number"
                    min={1}
                    max={5}
                    value={draft.retry.maxAttempts}
                    onChange={(event) =>
                      commit((previous) => ({
                        ...previous,
                        retry: {
                          ...previous.retry,
                          maxAttempts: Number(event.target.value) || 1,
                        },
                      }))
                    }
                  />
                  <span className="unit">{t("settings.attemptsUnit")}</span>
                </div>
                <span className="hint">{t("settings.maxAttemptsHint")}</span>
              </div>

              <div className="settings-field compact-threshold-field">
                <label htmlFor="retry-base-delay">{t("settings.baseDelay")}</label>
                <div className="inline-value">
                  <input
                    id="retry-base-delay"
                    className="text-input mono"
                    type="number"
                    min={100}
                    step={100}
                    value={draft.retry.baseDelayMs}
                    onChange={(event) =>
                      commit((previous) => ({
                        ...previous,
                        retry: {
                          ...previous.retry,
                          baseDelayMs: Number(event.target.value) || 100,
                        },
                      }))
                    }
                  />
                  <span className="unit">{t("settings.milliseconds")}</span>
                </div>
                <span className="hint">
                  {t("settings.baseDelayHint")}
                </span>
              </div>

              <div className="settings-field compact-threshold-field">
                <label htmlFor="retry-max-delay">{t("settings.maxDelay")}</label>
                <div className="inline-value">
                  <input
                    id="retry-max-delay"
                    className="text-input mono"
                    type="number"
                    min={1000}
                    step={1000}
                    value={draft.retry.maxDelayMs}
                    onChange={(event) =>
                      commit((previous) => ({
                        ...previous,
                        retry: {
                          ...previous.retry,
                          maxDelayMs: Number(event.target.value) || 1000,
                        },
                      }))
                    }
                  />
                  <span className="unit">{t("settings.milliseconds")}</span>
                </div>
                <span className="hint">
                  {t("settings.maxDelayHint")}
                </span>
              </div>
            </section>
          )}

          {section === "providers" && (
            <section className="surface settings-block">
              <span className="label">
                {editing
                  ? editing === "new"
                    ? t("settings.addProvider")
                    : t("settings.editProvider", { name: editing.name })
                  : picking
                    ? t("settings.chooseProvider")
                    : t("settings.providers")}
              </span>
              {editing === null && !picking && (
                <>
                  <div className="settings-provider-list">
                    {draft.providers.map((provider) => {
                      const status = keyStatus(provider, t);
                      return (
                        <div
                          className={`provider-row${draft.defaultProviderId === provider.id ? " is-default" : ""}`}
                          key={provider.id}
                        >
                          <span
                            className="provider-monogram"
                            aria-hidden="true"
                          >
                            {provider.name.slice(0, 1).toUpperCase()}
                          </span>
                          <div className="info">
                            <div className="p-name">
                              {provider.name}
                              {draft.defaultProviderId === provider.id && (
                                <span className="badge provider-default">
                                  {t("settings.defaultBadge")}
                                </span>
                              )}
                            </div>
                            <div className="caption p-url mono">{provider.baseUrl}</div>
                            <div className="p-badges">
                              <span className="badge neutral">
                                {provider.apiMode === "auto"
                                  ? t("settings.apiPerModel", { mode: API_MODE_LABELS.auto })
                                  : t("settings.apiFixed", { api: API_LABELS[provider.api] })}
                              </span>
                              <span
                                className={`badge ${status.warn ? "warn" : "neutral"}`}
                              >
                                {status.label}
                              </span>
                            </div>
                          </div>
                          <div className="p-actions">
                            <button
                              type="button"
                              className="link"
                              onClick={() => {
                                setPreset(null);
                                setPicking(false);
                                setEditing(provider);
                              }}
                            >
                              {t("common.edit")}
                            </button>
                            {draft.defaultProviderId !== provider.id && (
                              <button
                                type="button"
                                className="link"
                                onClick={() =>
                                  commit((previous) => ({
                                    ...previous,
                                    defaultProviderId: provider.id,
                                  }))
                                }
                              >
                                {t("settings.setDefault")}
                              </button>
                            )}
                            <button
                              type="button"
                              className="link danger"
                              onClick={() => deleteProvider(provider.id)}
                            >
                              {t("common.delete")}
                            </button>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                  {draft.providers.length === 0 && (
                    <p className="hint">
                      {t("settings.emptyProviders")}
                    </p>
                  )}
                </>
              )}

              {editing === null && !picking && (
                <div className="preset-actions">
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => setPicking(true)}
                  >
                    <IconPlus /> {t("settings.addFromPreset")}
                  </button>
                  <button
                    type="button"
                    className="link"
                    onClick={() => {
                      setPreset(null);
                      setEditing("new");
                    }}
                  >
                    {t("settings.manualEndpoint")}
                  </button>
                  <span className="spacer" />
                  <button
                    type="button"
                    className="link"
                    disabled={refreshingCatalog}
                    onClick={() => void refreshCatalog()}
                  >
                    <IconRefresh />
                    {refreshingCatalog ? t("settings.refreshingCatalog") : t("settings.refreshCatalog")}
                  </button>
                  {catalogNote && <span className="hint">{catalogNote}</span>}
                </div>
              )}

              {editing === null && picking && (
                <PresetPicker
                  existingIds={draft.providers.map((provider) => provider.id)}
                  onPick={(next) => {
                    setPreset(next);
                    setPicking(false);
                    setEditing("new");
                  }}
                  onCancel={() => setPicking(false)}
                />
              )}

              {editing !== null && (
                <ProviderForm
                  initial={editing === "new" ? null : editing}
                  preset={editing === "new" ? preset : null}
                  existingIds={draft.providers.map((provider) => provider.id)}
                  onSave={saveProvider}
                  onCancel={closeEditor}
                />
              )}
            </section>
          )}

          {section === "remote" && isTauriRuntime() && <RemoteAccessPanel />}

          {section === "account" && showLogout && (
            <section className="surface settings-block">
              <span className="label">{t("settings.remoteCredentials")}</span>
              <div className="provider-row">
                <div className="info">
                  <div className="p-name">
                    {t("settings.webAccessProtection")}
                    <span className="badge neutral">{t("settings.authenticated")}</span>
                  </div>
                  <div className="caption p-url mono">{t("settings.authTokenVerified")}</div>
                </div>
                <div className="p-actions">
                  <button
                    type="button"
                    className="link danger"
                    onClick={async () => {
                      await logout();
                      onClose();
                    }}
                  >
                    {t("settings.logout")}
                  </button>
                </div>
              </div>
            </section>
          )}
        </div>
      </div>
      </div>
      <SettingsRail controller={rail} section={section} settings={draft} t={t} showLogout={showLogout} onClose={onClose} />
    </div>
  );
}

function keyStatus(
  p: ProviderConfig,
  t: (key: string) => string,
): { label: string; warn: boolean } {
  if (p.envKey) return { label: `env: ${p.envKey}`, warn: false };
  if (p.apiKey) return { label: t("settings.keyStored"), warn: false };
  return { label: t("settings.noKey"), warn: true };
}
