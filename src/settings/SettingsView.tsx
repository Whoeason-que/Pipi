import { useEffect, useRef, useState } from "react";
import {
  catalogSourceLabel,
  type CatalogProvider
} from "../catalog";
import { loadCatalog, resetCatalog } from "../catalog-client";
import {
  formatRetryDelay
} from "../chat-runtime";
import {
  IconBack
} from "../icons";
import {
  logout
} from "../platform";
import {
  API_LABELS,
  type ProviderConfig,
  type Settings
} from "../types";

import { PresetPicker, ProviderForm } from "./ProviderEditor";
// ============ 设置（主区 tab，与对话界面同构：顶栏 + 左导航 + 内容） ============

/** 设置分组：导航用它划分不同类别。 */
type SettingsSection = "appearance" | "context" | "retry" | "providers" | "account";

const SETTINGS_SECTIONS: Record<SettingsSection, { label: string; description: string }> = {
  appearance: {
    label: "外观",
    description: "界面主题，只影响本机显示，不改动 Agent 配置。",
  },
  context: {
    label: "上下文",
    description: "长会话触碰窗口上限时的压缩方式：分叉出新会话，或原地替换旧轮次。",
  },
  retry: {
    label: "重试",
    description:
      "请求失败后的重发策略。只重发可重试的错误（限流、5xx、连接中断、流被截断）；"
      + "请求不合法、鉴权失败、上下文超限、配额耗尽一律不重发。已经输出正文的那一轮也不会重放。",
  },
  providers: {
    label: "模型提供商",
    description: "端点与密钥。Agent 绑定其中一个提供商，再选具体模型。",
  },
  account: {
    label: "账户",
    description: "Web 访问保护凭据。",
  },
};

interface SettingsViewProps {
  settings: Settings;
  onChange: (next: Settings) => void | Promise<void>;
  onClose: () => void;
  showLogout?: boolean;
}

export default function SettingsView({ settings, onChange, onClose, showLogout }: SettingsViewProps) {
  const [section, setSection] = useState<SettingsSection>("appearance");
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
      setCatalogNote(`已刷新：${next.providers.length} 家提供商 · 来源 ${catalogSourceLabel(next)}`);
    } catch (reason) {
      setCatalogNote(`刷新失败：${reason instanceof Error ? reason.message : String(reason)}`);
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
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

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
    if (!confirm(`删除提供商「${id}」？（已绑定它的 Agent 不受影响，但需重新配置）`)) return;
    commit((previous) => ({
      ...previous,
      providers: previous.providers.filter((provider) => provider.id !== id),
      defaultProviderId: previous.defaultProviderId === id ? null : previous.defaultProviderId,
    }));
  };

  const sections: SettingsSection[] = showLogout
    ? ["appearance", "context", "retry", "providers", "account"]
    : ["appearance", "context", "retry", "providers"];

  /** 导航项右侧的一行状态摘要，让分组一眼可辨。 */
  const sectionMeta = (id: SettingsSection): string => {
    switch (id) {
      case "appearance":
        return draft.theme === "dark" ? "深色" : "浅色";
      case "context":
        return draft.compaction.forkBeforeCompact ? "压缩前分叉" : "原地压缩";
      case "retry":
        return `${draft.retry.maxAttempts} 次尝试 · ${formatRetryDelay(draft.retry.baseDelayMs)}起`;
      case "providers": {
        const fallback = draft.providers.find((item) => item.id === draft.defaultProviderId);
        return `${draft.providers.length} 家 · 默认 ${fallback?.name ?? "未设置"}`;
      }
      case "account":
        return "Web 访问保护";
    }
  };

  return (
    <div className="screen settings-view">
      <div className="screen-bar">
        <button
          type="button"
          className="icon-btn"
          title="返回"
          aria-label="返回设置前的界面"
          onClick={onClose}
        >
          <IconBack />
        </button>
        <span className="crumb">
          设置 · <b>{SETTINGS_SECTIONS[section].label}</b>
        </span>
        <span className="spacer" />
        <span className="hint">改动即时保存</span>
      </div>

      <div className="settings-body">
        <nav
          className="settings-nav"
          role="tablist"
          aria-label="设置分组"
          aria-orientation="vertical"
        >
          {sections.map((id) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={section === id}
              className={`settings-nav-item${section === id ? " active" : ""}`}
              onClick={() => setSection(id)}
            >
              <span className="name">{SETTINGS_SECTIONS[id].label}</span>
              <span className="meta">{sectionMeta(id)}</span>
            </button>
          ))}
        </nav>

        <div className="settings-main">
          <div className="settings-main-head">
            <h2>{SETTINGS_SECTIONS[section].label}</h2>
            <p>{SETTINGS_SECTIONS[section].description}</p>
          </div>

          <div className="settings-main-body">
            {section === "appearance" && (
              <section className="settings-block">
                <span className="label">主题</span>
                <div className="theme-row">
                  <ThemeOption
                    active={draft.theme === "dark"}
                    name="深色"
                    swatch={["#171717", "#212121", "#0169CC", "#ececec"]}
                    onClick={() => commit((previous) => ({ ...previous, theme: "dark" }))}
                  />
                  <ThemeOption
                    active={draft.theme === "light"}
                    name="浅色"
                    swatch={["#f9f9f9", "#ffffff", "#0169CC", "#0d0d0d"]}
                    onClick={() => commit((previous) => ({ ...previous, theme: "light" }))}
                  />
                </div>
              </section>
            )}

            {section === "context" && (
              <section className="settings-block">
                <span className="label">上下文压缩</span>
                <div className="setting-checks">
                  <label className="ios-toggle">
                    <input
                      type="checkbox"
                      checked={draft.compaction.forkBeforeCompact}
                      onChange={() =>
                        commit((previous) => ({
                          ...previous,
                          compaction: {
                            ...previous.compaction,
                            forkBeforeCompact: !previous.compaction.forkBeforeCompact,
                          },
                        }))
                      }
                    />
                    <span className="slider" />
                    <span className="toggle-label">压缩前分叉新会话（原会话保留为完整记录）</span>
                  </label>
                  <label className="ios-toggle">
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
                    <span className="toggle-label">分叉后归档原会话</span>
                  </label>
                  <div className="hint">关闭分叉即回到原地压缩：摘要会替换当前会话里被压缩的旧轮次。</div>
                </div>
              </section>
            )}

            {section === "retry" && (
              <section className="settings-block">
                <span className="label">请求重试</span>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="retry-max-attempts">最大尝试次数</label>
                  <div className="inline-value">
                    <input
                      id="retry-max-attempts"
                      className="mono"
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
                      className="mono"
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
                  <span className="hint">第 n 次失败后等待 起始退避 × 2ⁿ⁻¹（带抖动）。</span>
                </div>

                <div className="settings-field compact-threshold-field">
                  <label htmlFor="retry-max-delay">退避上限</label>
                  <div className="inline-value">
                    <input
                      id="retry-max-delay"
                      className="mono"
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
                    服务端要求等待更久时直接放弃（不会静默等待）；无进展超时最多额外重试 1 次。
                  </span>
                </div>
              </section>
            )}

            {section === "providers" && (
              <section className="settings-block">
                <span className="label">模型提供商</span>
                {draft.providers.map((provider) => {
                  const status = keyStatus(provider);
                  return (
                    <div className="provider-row" key={provider.id}>
                      <div className="info">
                        <div className="p-name">
                          {provider.name}
                          <span className="badge neutral">{API_LABELS[provider.api]}</span>
                          <span className={`badge ${status.warn ? "warn" : "neutral"}`}>
                            {status.label}
                          </span>
                          {draft.defaultProviderId === provider.id && (
                            <span className="badge">默认</span>
                          )}
                        </div>
                        <div className="p-url mono">{provider.baseUrl}</div>
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
                            onClick={() => commit((previous) => ({ ...previous, defaultProviderId: provider.id }))}
                          >
                            设为默认
                          </button>
                        )}
                        <button type="button" className="link danger" onClick={() => deleteProvider(provider.id)}>
                          删除
                        </button>
                      </div>
                    </div>
                  );
                })}

                {editing === null && !picking && (
                  <div className="preset-actions">
                    <button type="button" className="btn ghost" onClick={() => setPicking(true)}>
                      ＋ 从预设添加
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
    </div>
  );
}

function ThemeOption({
  active,
  name,
  swatch,
  onClick,
}: {
  active: boolean;
  name: string;
  swatch: string[];
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`theme-option${active ? " active" : ""}`}
      aria-pressed={active}
      onClick={onClick}
    >
      <div className="swatch">
        {swatch.map((color) => (
          <span key={color} style={{ background: color }} />
        ))}
      </div>
      <div className="name">
        {name}
        {active && <span style={{ color: "var(--accent-text)" }}> ✓</span>}
      </div>
    </button>
  );
}

function keyStatus(p: ProviderConfig): { label: string; warn: boolean } {
  if (p.envKey) return { label: `env: ${p.envKey}`, warn: false };
  if (p.apiKey) return { label: "已存密钥", warn: false };
  return { label: "未配置密钥", warn: true };
}
