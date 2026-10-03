import { useEffect, useState } from "react";
import {
  catalogSourceLabel,
  providerGroups,
  providerSeed,
  type CatalogProvider,
  type ModelCatalog
} from "../catalog";
import { loadCatalog, peekCatalog } from "../catalog-client";
import { ChoiceSelect } from "../Select";
import {
  API_LABELS,
  type ApiKind,
  type ProviderConfig
} from "../types";


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
  const [catalog, setCatalog] = useState<ModelCatalog | null>(peekCatalog());
  const [loading, setLoading] = useState(peekCatalog() === null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (peekCatalog()) return;
    let alive = true;
    setLoading(true);
    loadCatalog(attempt > 0)
      .then((next) => {
        if (!alive) return;
        setCatalog(next);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (alive) setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [attempt]);

  const groups = catalog
    ? providerGroups(catalog).map((entry) => ({
        label: entry.label,
        options: entry.options.map((option) => {
          const provider = catalog.providers.find((item) => item.id === option.value);
          const parts = [option.label];
          if (provider?.api === "anthropic-messages") parts.push("Anthropic 协议");
          if (existingIds.includes(option.value)) parts.push("已添加");
          return { value: option.value, label: parts.join(" · ") };
        }),
      }))
    : [];

  return (
    <div className="preset-picker">
      <div className="preset-picker-head">
        <span className="label">选择提供商预设</span>
        <span className="spacer" />
        <button type="button" className="link" onClick={onCancel}>
          取消
        </button>
      </div>

      <div className="preset-picker-body">
        <ChoiceSelect
          value=""
          groups={groups}
          disabled={loading || !catalog}
          placeholder={loading ? "正在加载模型目录…" : "搜索提供商（名称 / id）…"}
          ariaLabel="选择提供商预设"
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
            <button type="button" className="link" onClick={() => setAttempt((n) => n + 1)}>
              重试
            </button>
          </div>
        )}
        {catalog && (
          <div className="hint">
            {catalog.providers.length} 家提供商 · 来源 {catalogSourceLabel(catalog)}；
            预设只填端点、协议与环境变量名，密钥始终由你提供（环境变量优先，其次明文）。
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
  const seed = preset ? providerSeed(preset) : null;
  const [name, setName] = useState(initial?.name ?? seed?.name ?? "");
  const [api, setApi] = useState<ApiKind>(initial?.api ?? seed?.api ?? "anthropic-messages");
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
            <span className="badge">目录预设</span>
            <span className="preset-banner-name">{preset.name}</span>
            <span className="badge neutral">{preset.group}</span>
            <span className="hint">
              {preset.models.length > 0
                ? `${preset.models.length} 个可工具调用的模型`
                : "本地运行时：模型名手填"}
            </span>
            {preset.doc && (
              <a className="link" href={preset.doc} target="_blank" rel="noreferrer">
                文档 ↗
              </a>
            )}
          </div>
          {preset.note && <div className="hint">{preset.note}</div>}
          {preset.baseUrlNote && (
            <div className="hint mono">端点说明：{preset.baseUrlNote}</div>
          )}
        </div>
      )}
      <div className="grid">
        <div className="form-row">
          <label className="label">名称</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="例如：DeepSeek"
            autoFocus
          />
          <div className="hint mono">{initial ? `id: ${id}` : `id: ${id || "…"}`}</div>
        </div>
        <div className="form-row">
          <label className="label">API 协议</label>
          <select value={api} onChange={(e) => setApi(e.target.value as ApiKind)}>
            {(Object.keys(API_LABELS) as ApiKind[]).map((k) => (
              <option key={k} value={k}>
                {API_LABELS[k]}
              </option>
            ))}
          </select>
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
          <label className="label">密钥环境变量（推荐）</label>
          <input
            className="mono"
            value={envKey}
            onChange={(e) => setEnvKey(e.target.value)}
            placeholder="DEEPSEEK_API_KEY"
          />
        </div>
        <div className="form-row">
          <label className="label">API Key（明文，本机自担）</label>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="留空则只用环境变量"
          />
        </div>
      </div>
      {idTaken && (
        <div className="form-warning">
          id「{id}」已存在：改一下名称即可换 id；或关闭本表单后直接「编辑」已有提供商。
        </div>
      )}
      <div className="actions">
        <button className="btn ghost" onClick={onCancel}>
          取消
        </button>
        <button className="btn primary" disabled={!valid} onClick={submit}>
          保存
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
