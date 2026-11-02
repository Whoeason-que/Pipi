import { useState } from "react";
import {
  catalogSourceLabel,
  PROVIDER_GROUP_LABELS,
  providerGroups,
  providerSeed,
  type CatalogProvider
} from "../catalog";
import { useI18n } from "../i18n";
import { ChoiceSelect } from "../Select";
import {
  API_LABELS,
  type ApiKind,
  type ApiMode,
  type ProviderConfig
} from "../types";
import { useCatalog } from "../use-catalog";


interface PresetPickerProps {
  existingIds: string[];
  onPick: (provider: CatalogProvider) => void;
  onCancel: () => void;
}

/**
 * 预设选择器：选项来自模型目录（models.dev），按分组列出。
 * 搜索与键盘导航交给 react-select；这里只标注「已添加」和来源。
 */
export function PresetPicker({ existingIds, onPick, onCancel }: PresetPickerProps) {
  const { t } = useI18n();
  const { catalog, loading, error, reload } = useCatalog();

  const groups = catalog
    ? providerGroups(catalog).map((entry) => ({
        label: PROVIDER_GROUP_LABELS[entry.label],
        options: entry.options.map((option) => {
          const provider = catalog.providers.find((item) => item.id === option.value);
          const parts = [option.label];
          if (provider?.api === "anthropic-messages") parts.push(t("settings.anthropicProtocol"));
          if (existingIds.includes(option.value)) parts.push(t("settings.alreadyAdded"));
          return { value: option.value, label: parts.join(" · ") };
        }),
      }))
    : [];

  return (
    <div className="preset-picker">
      <div className="preset-picker-head">
        <span className="label">{t("settings.choosePreset")}</span>
        <span className="spacer" />
        <button type="button" className="link" onClick={onCancel}>
          {t("common.cancel")}
        </button>
      </div>

      <div className="preset-picker-body">
        <ChoiceSelect
          value=""
          groups={groups}
          disabled={loading || !catalog}
          placeholder={loading ? t("settings.loadingCatalog") : t("settings.searchProviders")}
          ariaLabel={t("settings.choosePreset")}
          autoFocus
          menuInPortal
          onChange={(id) => {
            const hit = catalog?.providers.find((provider) => provider.id === id);
            if (hit) onPick(hit);
          }}
        />
        {error && (
          <div className="form-warning">
            {error}
            <button type="button" className="link" onClick={reload}>
              {t("common.retry")}
            </button>
          </div>
        )}
        {catalog && (
          <div className="hint">
            {t("settings.presetNote", {
              count: catalog.providers.length,
              source: catalogSourceLabel(catalog),
            })}
          </div>
        )}
      </div>
    </div>
  );
}

interface ProviderFormProps {
  initial: ProviderConfig | null;
  /** 从目录预设添加时的种子值（不含密钥）；编辑已有提供商时为 null。 */
  preset: CatalogProvider | null;
  existingIds: string[];
  onSave: (p: ProviderConfig) => void;
  onCancel: () => void;
}

export function ProviderForm({ initial, preset, existingIds, onSave, onCancel }: ProviderFormProps) {
  const { t } = useI18n();
  const seed = preset ? providerSeed(preset) : null;
  const [name, setName] = useState(initial?.name ?? seed?.name ?? "");
  const [api, setApi] = useState<ApiKind>(initial?.api ?? seed?.api ?? "anthropic-messages");
  const [apiMode, setApiMode] = useState<ApiMode>(initial?.apiMode ?? seed?.apiMode ?? "auto");
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? seed?.baseUrl ?? "");
  const [envKey, setEnvKey] = useState(initial?.envKey ?? seed?.envKey ?? "");
  const [apiKey, setApiKey] = useState(initial?.apiKey ?? "");

  // 预设默认沿用预设 id；但用户一改名称就改由名称派生（与手动路径一致），
  // 否则「改个名字换个 id」这条出路在预设路径上不成立。
  const nameEdited = Boolean(preset) && name.trim() !== preset!.name;
  const id = initial?.id ?? (seed && !nameEdited ? seed.id : slugify(name));

  const valid =
    name.trim().length > 0 &&
    baseUrl.trim().startsWith("http") &&
    (initial !== null || !existingIds.includes(id));

  const idTaken = initial === null && existingIds.includes(id);

  const submit = () => {
    if (!valid) return;
    onSave({
      id,
      name: name.trim(),
      api,
      apiMode,
      baseUrl: baseUrl.trim().replace(/\/+$/, ""),
      envKey: envKey.trim() || null,
      apiKey: apiKey.trim() || null,
    });
  };

  return (
    <div className="provider-form">
      {preset && (
        <div className="preset-banner">
          <div className="preset-banner-head">
            <span className="badge">{t("settings.catalogPreset")}</span>
            <span className="preset-banner-name">{preset.name}</span>
            <span className="badge neutral">{PROVIDER_GROUP_LABELS[preset.group]}</span>
            <span className="hint">
              {preset.models.length > 0
                ? t("settings.toolCapableModels", { count: preset.models.length })
                : t("settings.localRuntimeManualModels")}
            </span>
            {preset.doc && (
              <a className="link" href={preset.doc} target="_blank" rel="noreferrer">
                {t("settings.docs")}
              </a>
            )}
          </div>
          {preset.note && <div className="hint">{preset.note}</div>}
          {preset.baseUrlNote && (
            <div className="hint mono">{t("settings.endpointNote", { note: preset.baseUrlNote })}</div>
          )}
        </div>
      )}
      <div className="grid">
        <div className="form-row">
          <label className="label">{t("settings.name")}</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("settings.namePlaceholder")}
            autoFocus
          />
          <div className="hint mono">{initial ? `id: ${id}` : `id: ${id || "…"}`}</div>
        </div>
        <div className="form-row">
          <label className="label">{t("settings.apiProtocol")}</label>
          <select
            value={apiMode === "auto" ? "auto" : api}
            onChange={(e) => {
              const next = e.target.value;
              if (next === "auto") {
                setApiMode("auto");
              } else {
                setApiMode("manual");
                setApi(next as ApiKind);
              }
            }}
          >
            <option value="auto">{t("settings.apiAuto")}</option>
            {(Object.keys(API_LABELS) as ApiKind[]).map((k) => (
              <option key={k} value={k}>
                {API_LABELS[k]}
              </option>
            ))}
          </select>
          <div className="hint">
            {apiMode === "auto"
              ? t("settings.apiAutoHint", { api: API_LABELS[api] })
              : t("settings.apiManualHint", { api: API_LABELS[api] })}
          </div>
        </div>
        <div className="form-row full">
          <label className="label">Base URL</label>
          <input
            className="mono"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://api.deepseek.com/v1"
          />
        </div>
        <div className="form-row">
          <label className="label">{t("settings.envKey")}</label>
          <input
            className="mono"
            value={envKey}
            onChange={(e) => setEnvKey(e.target.value)}
            placeholder="DEEPSEEK_API_KEY"
          />
        </div>
        <div className="form-row">
          <label className="label">{t("settings.apiKeyPlain")}</label>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={t("settings.apiKeyPlaceholder")}
          />
        </div>
      </div>
      {idTaken && (
        <div className="form-warning">
          {t("settings.idTaken", { id })}
        </div>
      )}
      <div className="actions">
        <button className="btn ghost" onClick={onCancel}>
          {t("common.cancel")}
        </button>
        <button className="btn primary" disabled={!valid} onClick={submit}>
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}

function slugify(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\u4e00-\u9fff]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "provider-new";
}
