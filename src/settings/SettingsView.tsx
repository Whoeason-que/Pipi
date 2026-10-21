import { useContext, useEffect, useRef, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import { NavigationToggle } from "../app/AppNavigation";
import { catalogSourceLabel, type CatalogProvider } from "../catalog";
import { loadCatalog, resetCatalog } from "../catalog-client";
import { IconBack, IconLayers, IconPlus, IconRefresh } from "../icons";
import RemoteAccessPanel from "./RemoteAccessPanel";
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
        `已刷新：${next.providers.length} 家提供商 · 来源 ${catalogSourceLabel(next)}`,
      );
    } catch (reason) {
      setCatalogNote(
        `刷新失败：${reason instanceof Error ? reason.message : String(reason)}`,
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
        `删除提供商「${id}」？（已绑定它的 Agent 不受影响，但需重新配置）`,
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
    <div className="screen settings-view">
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
        <span className="crumb">
          {t("settings.title")} · <b>{sectionInfo.label}</b>
        </span>
        <span className="spacer" />
        {section !== "remote" && <span className="settings-autosave">
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
          <span className="settings-section-icon">
            <SettingsSectionIcon section={section} />
          </span>
          <div>
            <h2>{sectionInfo.label}</h2>
            <p>{sectionInfo.description}</p>
          </div>
        </div>

        <div className="settings-main-body">
          {section === "appearance" && (
            <>
              <section className="settings-block">
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

              <section className="settings-block">
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
            <section className="settings-block">
              <span className="label">上下文压缩</span>
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
                    <strong>压缩前分叉新会话</strong>
                    <span>保留原会话的完整记录，在新会话中继续。</span>
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
                    <strong>分叉后归档原会话</strong>
                    <span>收起原会话，让会话列表保持清晰。</span>
                  </span>
                </label>
                <div className="settings-info-note">
                  <IconLayers />
                  <span>
                    关闭分叉即回到原地压缩：摘要会替换当前会话里被压缩的旧轮次。
                  </span>
                </div>
              </div>
            </section>
          )}

          {section === "retry" && (
            <section className="settings-block retry-settings">
              <span className="label">请求重试</span>
              <div className="settings-field compact-threshold-field">
                <label htmlFor="retry-max-attempts">最大尝试次数</label>
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
                  <span className="unit">次（含首次）</span>
                </div>
                <span className="hint">1 = 不重试；上限 5。</span>
              </div>

              <div className="settings-field compact-threshold-field">
                <label htmlFor="retry-base-delay">起始退避</label>
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
                  <span className="unit">毫秒</span>
                </div>
                <span className="hint">
                  第 n 次失败后等待 起始退避 × 2ⁿ⁻¹（带抖动）。
                </span>
              </div>

              <div className="settings-field compact-threshold-field">
                <label htmlFor="retry-max-delay">退避上限</label>
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
                  <span className="unit">毫秒</span>
                </div>
                <span className="hint">
                  服务端要求等待更久时直接放弃（不会静默等待）；无进展超时最多额外重试
                  1 次。
                </span>
              </div>
            </section>
          )}

          {section === "providers" && (
            <section className="settings-block">
              <span className="label">
                {editing
                  ? editing === "new"
                    ? "添加提供商"
                    : `编辑 ${editing.name}`
                  : picking
                    ? "选择提供商"
                    : "模型提供商"}
              </span>
              {editing === null && !picking && (
                <>
                  <div className="settings-provider-list">
                    {draft.providers.map((provider) => {
                      const status = keyStatus(provider);
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
                                  默认
                                </span>
                              )}
                            </div>
                            <div className="p-url mono">{provider.baseUrl}</div>
                            <div className="p-badges">
                              <span className="badge neutral">
                                {provider.apiMode === "auto"
                                  ? `${API_MODE_LABELS.auto}（按模型）`
                                  : `${API_LABELS[provider.api]}（固定）`}
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
                              编辑
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
                                设为默认
                              </button>
                            )}
                            <button
                              type="button"
                              className="link danger"
                              onClick={() => deleteProvider(provider.id)}
                            >
                              删除
                            </button>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                  {draft.providers.length === 0 && (
                    <p className="hint">
                      添加一个模型提供商，为 Agent 选择可用的模型。
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
                    <IconPlus /> 从预设添加
                  </button>
                  <button
                    type="button"
                    className="link"
                    onClick={() => {
                      setPreset(null);
                      setEditing("new");
                    }}
                  >
                    手动配置端点
                  </button>
                  <span className="spacer" />
                  <button
                    type="button"
                    className="link"
                    disabled={refreshingCatalog}
                    onClick={() => void refreshCatalog()}
                  >
                    <IconRefresh />
                    {refreshingCatalog ? "刷新中…" : "刷新模型目录"}
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
            <section className="settings-block">
              <span className="label">远程访问凭据</span>
              <div className="provider-row">
                <div className="info">
                  <div className="p-name">
                    Web 访问保护
                    <span className="badge neutral">已认证</span>
                  </div>
                  <div className="p-url mono">PIPI_AUTH_TOKEN 已验证</div>
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
                    退出登录
                  </button>
                </div>
              </div>
            </section>
          )}
        </div>
      </div>
    </div>
  );
}

function keyStatus(p: ProviderConfig): { label: string; warn: boolean } {
  if (p.envKey) return { label: `env: ${p.envKey}`, warn: false };
  if (p.apiKey) return { label: "已存密钥", warn: false };
  return { label: "未配置密钥", warn: true };
}
