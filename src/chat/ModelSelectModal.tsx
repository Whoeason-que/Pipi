import { useRef, useState } from "react";
import { useDialogFocus } from "../components/use-dialog-focus";
import { errorMessage } from "../errors.ts";
import {
  catalogSourceLabel,
  findCatalogModel,
  matchProvider,
  modelApiOverride,
  pinnedApiConflict,
  resolveContextWindow,
  resolveMaxTokens,
} from "../catalog";
import {
  formatTokens
} from "../chat-runtime";
import { ModelPicker } from "../ModelPicker";
import { ReasoningSelect } from "../ReasoningSelect";
import { ChoiceSelect } from "../Select";
import { API_LABELS, pinnedProtocolHint, type ModelConfig, type ProviderConfig, type ReasoningEffort } from "../types";
import { useCatalog } from "../use-catalog";
import { useI18n } from "../i18n";


// ============ 会话模型选择 ============

interface ModelSelectModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: (target: { isCustom: boolean; model: ModelConfig | null }) => Promise<void>;
  currentModel: ModelConfig | null;
  isCustomModel: boolean;
  agentDefaultModel: ModelConfig | null;
  providers: ProviderConfig[];
}

export default function ModelSelectModal({
  isOpen,
  onClose,
  onConfirm,
  currentModel,
  isCustomModel,
  agentDefaultModel,
  providers,
}: ModelSelectModalProps) {
  const { t } = useI18n();
  const [mode, setMode] = useState<"default" | "custom">(isCustomModel ? "custom" : "default");

  const initialProvider = providers.find((p) => {
    if (isCustomModel && currentModel) {
      return p.api === currentModel.api && (p.baseUrl || "") === (currentModel.baseUrl || "");
    }
    if (agentDefaultModel) {
      return p.api === agentDefaultModel.api && (p.baseUrl || "") === (agentDefaultModel.baseUrl || "");
    }
    return false;
  }) ?? providers[0];

  const [selectedProviderId, setSelectedProviderId] = useState<string>(initialProvider?.id || "");
  const [modelId, setModelId] = useState<string>(
    (isCustomModel ? currentModel : agentDefaultModel)?.id ?? ""
  );
  const [maxTokens, setMaxTokens] = useState<number>(
    (isCustomModel ? currentModel : agentDefaultModel)?.maxTokens ?? 8192
  );
  const [contextWindow, setContextWindow] = useState<number>(
    (isCustomModel ? currentModel : agentDefaultModel)?.contextWindow ?? 0
  );
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort | null>(
    (isCustomModel ? currentModel : agentDefaultModel)?.reasoningEffort ?? null,
  );
  const [saving, setSaving] = useState(false);
  const [modalError, setModalError] = useState<string | null>(null);
  // 目录拉不到不影响切换模型：ModelPicker 会退化成手填，错误静默忽略
  const { catalog } = useCatalog();
  // 模态语义与焦点陷阱：此前这个弹窗没有 role/aria-modal，也没有键盘可达性。
  // Escape 先让给展开中的下拉菜单，否则一次按键会连菜单带弹窗一起关。
  const dialog = useRef<HTMLDivElement>(null);
  useDialogFocus(dialog, () => { if (!saving) onClose(); }, isOpen,
    () => !saving && !document.querySelector(".pipi-select__menu"));

  // 当前选中的供应商在目录里的条目（按「协议 + 端点」反查，口径与会话绑定一致）
  const activeProvider = providers.find((p) => p.id === selectedProviderId);
  const entry = matchProvider(catalog, activeProvider?.api ?? "", activeProvider?.baseUrl ?? "");

  if (!isOpen) return null;

  // 切换供应商必须清掉上一个供应商残留的模型 ID 与限额，
  // 否则会保存出「B 的端点 + A 的模型/上限」这种静默错配。
  const handleProviderChange = (nextProviderId: string) => {
    setSelectedProviderId(nextProviderId);
    setModelId("");
    setMaxTokens(8192);
    setContextWindow(0);
    setReasoningEffort(null);
  };
  const pickedModel = findCatalogModel(entry, modelId);
  // 目录标注的模型级协议：auto 时后端按它请求；钉死时只提醒、不改写。
  const providerApi = activeProvider?.api ?? "openai-completions";
  const apiMode = activeProvider?.apiMode ?? "auto";
  const modelOverride = modelApiOverride(pickedModel, apiMode);
  const pinnedConflict = pinnedApiConflict(pickedModel, providerApi, apiMode);

  const handleSave = async () => {
    setModalError(null);
    setSaving(true);
    try {
      if (mode === "default") {
        await onConfirm({ isCustom: false, model: agentDefaultModel });
      } else {
        const trimmed = modelId.trim();
        if (!trimmed) {
          setModalError(t("chat.modelIdRequired"));
          setSaving(false);
          return;
        }
        const provider = providers.find((p) => p.id === selectedProviderId);
        if (!provider) {
          setModalError(t("chat.providerRequired"));
          setSaving(false);
          return;
        }
        const targetModel: ModelConfig = {
          id: trimmed,
          name: trimmed,
          api: provider.api,
          baseUrl: provider.baseUrl || "",
          maxTokens: maxTokens > 0 ? maxTokens : 8192,
          contextWindow: contextWindow > 0 ? contextWindow : 0,
          reasoningEffort,
        };
        await onConfirm({ isCustom: true, model: targetModel });
      }
    } catch (err: unknown) {
      setModalError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget && !saving) onClose();
      }}
    >
      <div
        ref={dialog}
        className="modal model-select-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="model-select-title"
      >
        <div className="modal-header">
          <h2 id="model-select-title">{t("chat.selectSessionModel")}</h2>
          <button type="button" className="icon-btn close-btn" onClick={onClose} disabled={saving}>
            ✕
          </button>
        </div>

        <div className="modal-section">
          <div className="mode-toggle-group">
            <button
              type="button"
              className={`mode-toggle-card ${mode === "default" ? "active" : ""}`}
              onClick={() => setMode("default")}
            >
              <div className="mode-toggle-title">{t("chat.followAgentDefault")}</div>
              <div className="mode-toggle-desc">
                {agentDefaultModel ? (
                  <span className="mono">{agentDefaultModel.id}</span>
                ) : (
                  <span className="mode-toggle-hint">{t("chat.noAgentDefault")}</span>
                )}
              </div>
            </button>

            <button
              type="button"
              className={`mode-toggle-card ${mode === "custom" ? "active" : ""}`}
              onClick={() => setMode("custom")}
            >
              <div className="mode-toggle-title">{t("chat.customSessionModel")}</div>
              <div className="mode-toggle-desc">
                {t("chat.customSessionModelHint")}
              </div>
            </button>
          </div>
        </div>

        {mode === "custom" && (
          <div className="modal-section custom-model-form">
            <div className="form-row">
              <label className="label" htmlFor="model-provider-select">{t("chat.provider")}</label>
              {providers.length > 0 ? (
                <ChoiceSelect
                  id="model-provider-select"
                  value={selectedProviderId}
                  choices={providers.map((p) => ({
                    value: p.id,
                    label: t("chat.providerOption", {
                      name: p.name || p.id,
                      protocol: API_LABELS[p.api],
                    }),
                  }))}
                  onChange={handleProviderChange}
                  menuInPortal
                />
              ) : (
                <div className="form-warning">
                  {t("chat.noProviders")}
                </div>
              )}
            </div>

            <div className="form-row">
              <label className="label" htmlFor="session-model-id">{t("chat.model")}</label>
              <ModelPicker
                id="session-model-id"
                providers={providers}
                providerId={selectedProviderId}
                modelId={modelId}
                disabled={saving}
                placeholder={t("chat.modelPlaceholder")}
                onModelChange={(nextId, model) => {
                  if (nextId !== modelId) setReasoningEffort(null);
                  setModelId(nextId);
                  // 手填（model 为 undefined）时保留已有限额：不能因为改了个 ID 就静默把
                  // 目录/用户先前的 maxTokens、contextWindow 复位成默认值。
                  setMaxTokens(resolveMaxTokens(model, maxTokens));
                  setContextWindow(resolveContextWindow(model, contextWindow));
                }}
              />
            </div>

            <div className="form-row">
              <ReasoningSelect
                id="session-reasoning-effort"
                value={reasoningEffort}
                onChange={setReasoningEffort}
                disabled={saving || !modelId.trim() || !selectedProviderId}
              />
            </div>

            {entry && entry.models.length > 0 ? (
              <div className="form-row">
                <div className="hint">
                  {t("chat.catalogHint", {
                    provider: entry.name,
                    count: entry.models.length,
                    source: catalog
                      ? catalogSourceLabel(catalog)
                      : t("chat.catalogSourceFallback"),
                  })}
                </div>
                {pickedModel && (
                  <div className="hint mono">
                    {t("chat.selectedModelSummary", {
                      model: pickedModel.name,
                      context: formatTokens(pickedModel.context),
                      output: formatTokens(pickedModel.output),
                    })}
                    {modelOverride
                      ? ` · ${t("chat.selectedModelProtocol", {
                          protocol: API_LABELS[modelOverride],
                        })}`
                      : ""}
                  </div>
                )}
                {pinnedConflict && (
                  <div className="hint">{pinnedProtocolHint(pinnedConflict, providerApi)}</div>
                )}
              </div>
            ) : (
              providers.length > 0 && (
                <div className="form-row">
                  <div className="hint">
                    {t("chat.providerNotInCatalog")}
                  </div>
                </div>
              )
            )}
          </div>
        )}

        {modalError && <div className="form-error-banner">{modalError}</div>}

        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onClose} disabled={saving}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={handleSave}
            disabled={saving || (mode === "custom" && (!modelId.trim() || !selectedProviderId))}
          >
            {saving ? t("chat.switching") : t("chat.confirmSwitch")}
          </button>
        </div>
      </div>
    </div>
  );
}
