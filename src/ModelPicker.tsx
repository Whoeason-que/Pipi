import {
  catalogSourceLabel,
  fallbackModelChoices,
  findCatalogModel,
  matchProvider,
  modelApiOverride,
  modelChoices,
  pinnedApiConflict,
  type CatalogModel,
} from "./catalog";
import { ChoiceCreatable } from "./Select";
import { useI18n } from "./i18n";
import { API_LABELS, pinnedProtocolHint, type ProviderConfig } from "./types";
import { useCatalog } from "./use-catalog";

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
  const { t } = useI18n();
  const provider = providers.find((p) => p.id === providerId);
  const { catalog, loading, error } = useCatalog();

  const entry = matchProvider(catalog, provider?.api ?? "", provider?.baseUrl ?? "");
  const choices = entry ? modelChoices(entry) : catalog ? [] : fallbackModelChoices();

  // 目录标注的模型级协议：auto 时后端按它请求；钉死时只提醒、不改写。
  const providerApi = provider?.api ?? "openai-completions";
  const apiMode = provider?.apiMode ?? "auto";
  const selectedModel = findCatalogModel(entry, modelId);
  const modelOverride = modelApiOverride(selectedModel, apiMode);
  const pinnedConflict = pinnedApiConflict(selectedModel, providerApi, apiMode);

  const hint = (() => {
    if (loading) return t("app.catalogLoading");
    if (error) return t("app.catalogError", { error });
    if (!catalog) return t("app.catalogOffline");
    if (!providerId) return t("app.selectProviderFirst");
    if (!entry) return t("app.providerNotInCatalog");
    const source = catalogSourceLabel(catalog);
    const note = entry.baseUrlNote ? ` (${entry.baseUrlNote})` : "";
    return `${t("app.catalogSummary", {
      name: entry.name,
      count: entry.models.length,
      source,
    })}${note}`;
  })();

  return (
    <div className="model-picker">
      <ChoiceCreatable
        id={id}
        value={modelId}
        choices={choices}
        // 加载中不禁用：离线/弱网时用户仍要能手填模型 ID（禁用等于把退化路径堵死）
        disabled={disabled}
        placeholder={loading ? t("app.catalogLoadingPlaceholder") : placeholder ?? t("app.modelPlaceholder")}
        onChange={(next) => onModelChange(next, findCatalogModel(entry, next))}
        createLabel={(input) => t("app.useCustomModelId", { input })}
        menuInPortal
      />
      <span className="hint">{hint}</span>
      {modelOverride && (
        <span className="hint">
          {t("app.modelProtocolHint", { api: API_LABELS[modelOverride] })}
        </span>
      )}
      {pinnedConflict && (
        <span className="hint">{pinnedProtocolHint(pinnedConflict, providerApi)}</span>
      )}
    </div>
  );
}
