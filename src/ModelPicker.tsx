import { useEffect, useState } from "react";
import {
  catalogSourceLabel,
  fallbackModelChoices,
  findCatalogModel,
  matchProvider,
  modelChoices,
  type CatalogModel,
  type ModelCatalog,
} from "./catalog";
import { loadCatalog, peekCatalog } from "./catalog-client";
import { ChoiceCreatable } from "./Select";
import type { ProviderConfig } from "./types";

interface ModelPickerProps {
  /** 已配置的供应商（settings.providers） */
  providers: ProviderConfig[];
  /** 当前供应商 id（空串 = 未绑定） */
  providerId: string;
  modelId: string;
  /** 选中模型后回调；手填的模型没有目录元数据，`model` 为 undefined */
  onModelChange: (modelId: string, model?: CatalogModel) => void;
  /** DOM id：label htmlFor 与冒烟断言都用它 */
  id: string;
  disabled?: boolean;
  placeholder?: string;
}

/**
 * 模型选择器：选项来自模型目录（models.dev → pipi-core → 本地缓存）。
 * 目录里没有的模型（自建端点、未收录模型）直接输入回车即可创建 —— 不阻塞任何用法。
 */
export function ModelPicker({
  providers,
  providerId,
  modelId,
  onModelChange,
  id,
  disabled,
  placeholder,
}: ModelPickerProps) {
  const provider = providers.find((p) => p.id === providerId);
  const [catalog, setCatalog] = useState<ModelCatalog | null>(peekCatalog());
  const [loading, setLoading] = useState(peekCatalog() === null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (peekCatalog()) return;
    let alive = true;
    setLoading(true);
    loadCatalog()
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
  }, []);

  const entry = matchProvider(catalog, provider?.api ?? "", provider?.baseUrl ?? "");
  const choices = entry ? modelChoices(entry) : catalog ? [] : fallbackModelChoices();

  const hint = (() => {
    if (loading) return "正在加载模型目录（models.dev）…";
    if (error) return `模型目录不可用：${error} —— 直接输入模型 ID，端点以你绑定的供应商为准。`;
    if (!catalog) return "模型目录不可用（离线或拉取失败）：直接输入模型 ID，端点以你绑定的供应商为准。";
    if (!providerId) return "先选择供应商；也可以先填模型 ID，稍后再绑定。";
    if (!entry) return "该供应商不在模型目录里（自定义端点）：直接输入模型 ID。";
    const source = catalogSourceLabel(catalog);
    const note = entry.baseUrlNote ? `（${entry.baseUrlNote}）` : "";
    return `${entry.name}：目录收录 ${entry.models.length} 个可工具调用的模型 · 来源 ${source}${note}`;
  })();

  return (
    <div className="model-picker">
      <ChoiceCreatable
        id={id}
        value={modelId}
        choices={choices}
        // 加载中不禁用：离线/弱网时用户仍要能手填模型 ID（禁用等于把退化路径堵死）
        disabled={disabled}
        placeholder={loading ? "正在加载模型目录（也可直接输入模型 ID）…" : placeholder ?? "选择或输入模型 ID"}
        onChange={(next) => onModelChange(next, findCatalogModel(entry, next))}
        createLabel={(input) => `使用自定义模型 ID：${input}`}
        menuInPortal
      />
      <span className="hint">{hint}</span>
    </div>
  );
}
