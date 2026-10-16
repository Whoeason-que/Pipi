import { useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import {
  resolveContextWindow,
  resolveMaxTokens
} from "../catalog";
import {
  formatRuntimeError
} from "../chat-runtime";
import {
  IconBack
} from "../icons";
import { ModelPicker } from "../ModelPicker";
import {
  invoke
} from "../platform";
import { ChoiceSelect } from "../Select";
import {
  type ProviderConfig
} from "../types";
import { DEFAULT_TOOLS, permissionsFromFields } from "./agent-fields";
import { BashChoices, SandboxChoices, ToolChoices } from "./PermissionChoices";
import { useAgentConfigFields } from "./use-agent-config-fields";

// ============ 新建 Agent ============

interface CreateAgentFormProps {
  providers: ProviderConfig[];
  defaultProviderId: string | null;
  onCreated: (name: string) => void | Promise<void>;
  onCancel: () => void;
  onError: (msg: string | null) => void;
}

export default function CreateAgentForm({
  providers,
  defaultProviderId,
  onCreated,
  onCancel,
  onError,
}: CreateAgentFormProps) {
  const [name, setName] = useState("");
  const fields = useAgentConfigFields({
    providerId: defaultProviderId && providers.some((p) => p.id === defaultProviderId)
      ? defaultProviderId
      : providers[0]?.id ?? "",
    modelId: "",
    description: "",
    workspace: "",
    // Agent 组合工具具有跨 Agent 的持久副作用，必须显式勾选；新建 Agent
    // 继续只默认启用原来的基础工具。
    tools: [...DEFAULT_TOOLS],
    bashMode: "allowAll",
    commands: "",
    sandbox: "workspace-write",
  });
  const [submitting, setSubmitting] = useState(false);

  const submit = async () => {
    if (!name.trim() || submitting) return;
    setSubmitting(true);
    onError(null);
    try {
      const permissions = permissionsFromFields(
        fields.tools,
        fields.bashMode,
        fields.commands,
        fields.sandbox,
      );

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
        <button type="button" className="icon-btn" title="返回" onClick={onCancel}>
          <IconBack />
        </button>
        <span className="crumb">
          新建 Agent
        </span>
      </div>
      <div className="detail">
        <div className="form">
          <div>
            <h2>新建 Agent</h2>
            <div className="sub">
              给 Agent 一个名字，选择模型与工作目录。指令、技能和记忆会随它一起保留。
            </div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-name">名称</label>
            <input
              id="agent-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="例如：code-reviewer"
              autoFocus
            />
            <div className="hint">仅限字母、数字、- 和 _，将作为目录名</div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-desc">描述</label>
            <textarea
              id="agent-desc"
              value={fields.description}
              onChange={(e) => fields.setDescription(e.target.value)}
              placeholder="这个 Agent 是做什么的？"
            />
          </div>

          <div className="field">
            <label className="label">默认模型</label>
            <div className="bind-row">
              <div className="provider-picker">
                <ChoiceSelect
                  id="agent-create-provider"
                  value={fields.providerId}
                  choices={providers.map((p) => ({ value: p.id, label: p.name }))}
                  placeholder="（未绑定提供商）"
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
                placeholder="模型 ID，如 claude-sonnet-4-5 / gpt-4o"
                onModelChange={fields.changeModel}
              />
            </div>
            <div className="hint">新建会话时将默认使用此模型；也可留空稍后在详情页配置</div>
          </div>

          <div className="field">
            <label className="label" htmlFor="agent-workspace">工作目录</label>
            <input
              id="agent-workspace"
              className="mono"
              value={fields.workspace}
              onChange={(e) => fields.setWorkspace(e.target.value)}
              placeholder="例如：~/projects/my-app"
            />
            <div className="hint">
              Agent 只在此目录内工作；留空使用默认的 agent 目录内 workspace/
            </div>
          </div>

          <div className="field">
            <span className="label">工具</span>
            <ToolChoices value={fields.tools} onToggle={fields.toggleTool} />
          </div>

          <div className="field">
            <span className="label">沙箱</span>
            <SandboxChoices value={fields.sandbox} onChange={fields.setSandbox} name="sandbox" />
            <div className="hint">
              只读：不执行命令、不写文件；工作目录内可写：强制删除类命令与越出
              工作目录的写入被拒绝；完全访问：不设限
            </div>
          </div>

          <div className="field">
            <span className="label">命令权限（bash）</span>
            <BashChoices value={fields.bashMode} onChange={fields.setBashMode} name="bash-mode" />
            {fields.bashMode !== "allowAll" && (
              <div className="perm-editor">
                <textarea
                  value={fields.commands}
                  onChange={(e) => fields.setCommands(e.target.value)}
                  placeholder={fields.bashMode === "allowlist" ? "git\nnpm run\nls" : "rm\nsudo"}
                />
                <div className="hint">
                  {fields.bashMode === "allowlist"
                    ? "每行一条；单词条目匹配以该词开头的命令，带空格按前缀匹配"
                    : "每行一条；命中任意条目的命令将被拒绝"}
                </div>
              </div>
            )}
          </div>

          <div className="actions">
            <button className="btn ghost" onClick={onCancel}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={!name.trim() || fields.tools.length === 0 || submitting}
              onClick={submit}
            >
              {submitting ? "创建中…" : "创建"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
