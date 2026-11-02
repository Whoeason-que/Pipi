import { useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import {
  resolveContextWindow,
  resolveMaxTokens
} from "../catalog";
import {
  formatRuntimeError
} from "../chat-runtime";
import { useI18n } from "../i18n";
import {
  IconBack
} from "../icons";
import { ModelPicker } from "../ModelPicker";
import { ReasoningSelect } from "../ReasoningSelect";
import {
  invoke
} from "../platform";
import { ChoiceSelect } from "../Select";
import {
  type ProviderConfig
} from "../types";
import { DEFAULT_TOOLS, permissionsFromFields } from "./agent-fields";
import { SandboxChoices, ToolChoices } from "./PermissionChoices";
import { useAgentConfigFields } from "./use-agent-config-fields";

// ============ 新建 Agent ============

interface CreateAgentFormProps {
  providers: ProviderConfig[];
  defaultProviderId: string | null;
  defaultWorkspace?: string | null;
  onCreated: (name: string) => void | Promise<void>;
  onCancel: () => void;
  onError: (msg: string | null) => void;
}

export default function CreateAgentForm({
  providers,
  defaultProviderId,
  defaultWorkspace,
  onCreated,
  onCancel,
  onError,
}: CreateAgentFormProps) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const fields = useAgentConfigFields({
    providerId: defaultProviderId && providers.some((p) => p.id === defaultProviderId)
      ? defaultProviderId
      : providers[0]?.id ?? "",
    modelId: "",
    description: "",
    workspace: defaultWorkspace ?? "",
    // Agent 组合工具具有跨 Agent 的持久副作用，必须显式勾选；新建 Agent
    // 继续只默认启用原来的基础工具。
    tools: [...DEFAULT_TOOLS],
    sandbox: "workspace-write",
  });
  const [submitting, setSubmitting] = useState(false);

  const submit = async () => {
    if (!name.trim() || submitting) return;
    setSubmitting(true);
    onError(null);
    try {
      const permissions = permissionsFromFields(fields.tools, fields.sandbox);

      const selectedProvider = providers.find((p) => p.id === fields.providerId);
      const trimmedModel = fields.modelId.trim();
      const provider = selectedProvider && trimmedModel
        ? {
            id: trimmedModel,
            name: trimmedModel,
            api: selectedProvider.api,
            baseUrl: selectedProvider.baseUrl,
            maxTokens: resolveMaxTokens(fields.pickedModel, 8192),
            contextWindow: resolveContextWindow(fields.pickedModel, 0),
            reasoningEffort: fields.reasoningEffort,
          }
        : null;

      await invoke("create_agent", {
        name,
        description: fields.description,
        workspace: fields.workspace.trim() || null,
        permissions,
        model: trimmedModel || null,
        provider,
      });
      await onCreated(name.trim());
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="screen">
      <div className="screen-bar">
        <NavigationToggle />
        <button
          type="button"
          className="icon-btn"
          title={t("common.back")}
          onClick={onCancel}
        >
          <IconBack />
        </button>
        <span className="truncate caption crumb">
          {t("nav.newAgent")}
        </span>
      </div>
      <div className="detail">
        <div className="form">
          <div>
            <h2>{t("nav.newAgent")}</h2>
            <div className="sub">{t("agents.createSubtitle")}</div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-name">
              {t("agents.name")}
            </label>
            <input
              id="agent-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("agents.namePlaceholder")}
              autoFocus
            />
            <div className="hint">{t("agents.nameHint")}</div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-desc">
              {t("agents.description")}
            </label>
            <textarea
              id="agent-desc"
              value={fields.description}
              onChange={(e) => fields.setDescription(e.target.value)}
              placeholder={t("agents.descriptionPlaceholder")}
            />
          </div>

          <div className="field">
            <label className="label">{t("workspace.defaultModel")}</label>
            <div className="bind-row">
              <div className="provider-picker">
                <ChoiceSelect
                  id="agent-create-provider"
                  value={fields.providerId}
                  choices={providers.map((p) => ({ value: p.id, label: p.name }))}
                  placeholder={t("agents.noProvider")}
                  isClearable
                  menuInPortal
                  onChange={fields.changeProvider}
                />
              </div>
              <ModelPicker
                id="agent-create-model"
                providers={providers}
                providerId={fields.providerId}
                modelId={fields.modelId}
                placeholder={t("agents.modelIdPlaceholderWithPresets")}
                onModelChange={fields.changeModel}
              />
            </div>
            <div className="hint">{t("agents.createModelHint")}</div>
          </div>

          <div className="field">
            <ReasoningSelect
              id="agent-create-reasoning-effort"
              value={fields.reasoningEffort}
              onChange={fields.setReasoningEffort}
              disabled={submitting || !fields.providerId || !fields.modelId.trim()}
            />
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-workspace">
              {t("workspace.directory")}
            </label>
            <input
              id="agent-workspace"
              className="mono"
              value={fields.workspace}
              onChange={(e) => fields.setWorkspace(e.target.value)}
              placeholder={t("agents.workspaceExamplePlaceholder")}
            />
            <div className="hint">{t("agents.createWorkspaceHint")}</div>
          </div>

          <div className="field">
            <span className="label">{t("agents.tools")}</span>
            <ToolChoices value={fields.tools} onToggle={fields.toggleTool} />
          </div>

          <div className="field">
            <span className="label">{t("agents.sandbox")}</span>
            <SandboxChoices value={fields.sandbox} onChange={fields.setSandbox} name="sandbox" />
            <div className="hint">{t("agents.sandboxHintCreate")}</div>
          </div>

          <div className="actions">
            <button className="btn ghost" onClick={onCancel}>
              {t("common.cancel")}
            </button>
            <button
              className="btn primary"
              disabled={!name.trim() || fields.tools.length === 0 || submitting}
              onClick={submit}
            >
              {submitting ? t("agents.creating") : t("common.create")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
