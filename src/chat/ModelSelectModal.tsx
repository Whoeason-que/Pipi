import { useEffect, useState } from "react";
import {
  catalogSourceLabel,
  findCatalogModel,
  matchProvider,
  resolveContextWindow,
  resolveMaxTokens,
  type ModelCatalog,
} from "../catalog";
import { loadCatalog, peekCatalog } from "../catalog-client";
import {
  formatTokens
} from "../chat-runtime";
import { ModelPicker } from "../ModelPicker";
import { ChoiceSelect } from "../Select";
import type { ModelConfig, ProviderConfig } from "../types";


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
    isCustomModel && currentModel ? currentModel.id : ""
  );
  const [maxTokens, setMaxTokens] = useState<number>(
    isCustomModel && currentModel ? currentModel.maxTokens : 8192
  );
  const [contextWindow, setContextWindow] = useState<number>(
    isCustomModel && currentModel ? currentModel.contextWindow : 0
  );
  const [saving, setSaving] = useState(false);
  const [modalError, setModalError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<ModelCatalog | null>(peekCatalog());

  useEffect(() => {
    if (peekCatalog()) return;
    let alive = true;
    loadCatalog()
      .then((next) => {
        if (alive) setCatalog(next);
      })
      .catch(() => {
        // 目录拉不到不影响切换模型：ModelPicker 会退化成手填
      });
    return () => {
      alive = false;
    };
  }, []);

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
  };
  const pickedModel = findCatalogModel(entry, modelId);

  const handleSave = async () => {
    setModalError(null);
    setSaving(true);
    try {
      if (mode === "default") {
        await onConfirm({ isCustom: false, model: agentDefaultModel });
      } else {
        const trimmed = modelId.trim();
        if (!trimmed) {
          setModalError("请输入模型 ID（如 claude-3-7-sonnet-20250219）");
          setSaving(false);
          return;
        }
        const provider = providers.find((p) => p.id === selectedProviderId);
        if (!provider) {
          setModalError("请选择有效的供应商配置");
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
        };
        await onConfirm({ isCustom: true, model: targetModel });
      }
    } catch (err: unknown) {
      setModalError(err instanceof Error ? err.message : String(err));
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
      <div className="modal model-select-modal">
        <div className="modal-header">
          <h2>选择会话模型</h2>
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
              <div className="mode-toggle-title">跟随 Agent 默认</div>
              <div className="mode-toggle-desc">
                {agentDefaultModel ? (
                  <span className="mono">{agentDefaultModel.id}</span>
                ) : (
                  <span className="mode-toggle-hint">（Agent 暂未绑定默认模型）</span>
                )}
              </div>
            </button>

            <button
              type="button"
              className={`mode-toggle-card ${mode === "custom" ? "active" : ""}`}
              onClick={() => setMode("custom")}
            >
              <div className="mode-toggle-title">自定义会话模型</div>
              <div className="mode-toggle-desc">
                仅对当前会话生效，后续可随时切换
              </div>
            </button>
          </div>
        </div>

        {mode === "custom" && (
          <div className="modal-section custom-model-form">
            <div className="form-row">
              <label className="label" htmlFor="model-provider-select">供应商</label>
              {providers.length > 0 ? (
                <ChoiceSelect
                  id="model-provider-select"
                  value={selectedProviderId}
                  choices={providers.map((p) => ({
                    value: p.id,
                    label: `${p.name || p.id}（${p.api === "anthropic-messages" ? "Anthropic" : "OpenAI"} 协议）`,
                  }))}
                  onChange={handleProviderChange}
                  menuInPortal
                />
              ) : (
                <div className="form-warning">
                  未检测到配置的供应商，请先在右上角「设置」中添加供应商 API Key。
                </div>
              )}
            </div>

            <div className="form-row">
              <label className="label" htmlFor="session-model-id">模型</label>
              <ModelPicker
                id="session-model-id"
                providers={providers}
                providerId={selectedProviderId}
                modelId={modelId}
                disabled={saving}
                placeholder="如 gpt-4o、deepseek-chat、anthropic/claude-sonnet-4.5"
                onModelChange={(nextId, model) => {
                  setModelId(nextId);
                  // 手填（model 为 undefined）时保留已有限额：不能因为改了个 ID 就静默把
                  // 目录/用户先前的 maxTokens、contextWindow 复位成默认值。
                  setMaxTokens(resolveMaxTokens(model, maxTokens));
                  setContextWindow(resolveContextWindow(model, contextWindow));
                }}
              />
            </div>

            {entry && entry.models.length > 0 ? (
              <div className="form-row">
                <div className="hint">
                  {`模型列表来自 models.dev：${entry.name} 收录 ${entry.models.length} 个可工具调用的模型（来源 ${catalog ? catalogSourceLabel(catalog) : "目录"}）；目录外的模型直接输入模型 ID 回车即可。`}
                </div>
                {pickedModel && (
                  <div className="hint mono">
                    已选 {pickedModel.name} · 上下文 {formatTokens(pickedModel.context)} · 最大输出{" "}
                    {formatTokens(pickedModel.output)}
                  </div>
                )}
              </div>
            ) : (
              providers.length > 0 && (
                <div className="form-row">
                  <div className="hint">
                    该供应商不在模型目录里（自定义端点）：直接输入模型 ID 即可。
                  </div>
                </div>
              )
            )}
          </div>
        )}

        {modalError && <div className="form-error-banner">{modalError}</div>}

        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onClose} disabled={saving}>
            取消
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={handleSave}
            disabled={saving || (mode === "custom" && (!modelId.trim() || !selectedProviderId))}
          >
            {saving ? "切换中…" : "确认切换"}
          </button>
        </div>
      </div>
    </div>
  );
}
