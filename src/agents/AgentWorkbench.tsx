import { useEffect, useRef, useState } from "react";
import {
  resolveContextWindow,
  resolveMaxTokens,
  type CatalogModel
} from "../catalog";
import ChatView from "../Chat";
import {
  formatRuntimeError
} from "../chat-runtime";
import { SessionDiagnostics } from "../chat/Inspector";
import {
  IconBack
} from "../icons";
import { ModelPicker } from "../ModelPicker";
import {
  invoke
} from "../platform";
import { ChoiceSelect } from "../Select";
import type { SessionRuntime } from "../session-runtime";
import {
  type AgentDefinition,
  type BashMode,
  type ProviderConfig,
  type SandboxMode,
  type SessionInfoView
} from "../types";
import { permissionsFromFields } from "./agent-fields";
import { BashChoices, SandboxChoices, ToolChoices } from "./PermissionChoices";

import AgentFileEditor from "./AgentFileEditor";
// ============ Agent 详情 ============

interface AgentWorkbenchProps {
  agent: AgentDefinition;
  providers: ProviderConfig[];
  onSaved: (subagent?: boolean) => void | Promise<void>;
  onBack: () => void;
  onError: (msg: string) => void;
  blockedSessionIds: string[];
  sessions: SessionRuntime;
  onRunningChange: (agentName: string, sessionId: string | null, running: boolean) => void;
}

type AgentSettingsTab = "basic" | "permissions" | "files" | "test";

export default function AgentWorkbench({
  agent,
  providers,
  onSaved,
  onBack,
  onError,
  blockedSessionIds,
  sessions,
  onRunningChange,
}: AgentWorkbenchProps) {
  const { bash } = agent.permissions;
  const [testSession, setTestSession] = useState<SessionInfoView | null>(null);
  const [testLoadError, setTestLoadError] = useState<string | null>(null);
  const [testLoadVersion, setTestLoadVersion] = useState(0);
  const [settingsTab, setSettingsTab] = useState<AgentSettingsTab>("basic");
  const [providerId, setProviderId] = useState<string>(() => {
    const bound = providers.find(
      (p) => agent.provider && p.api === agent.provider.api && p.baseUrl === agent.provider.baseUrl,
    );
    return bound?.id ?? "";
  });
  const [modelId, setModelId] = useState(agent.provider?.id ?? "");
  // 从目录里选的模型：用它回填 maxTokens / contextWindow（不在目录里的模型保持原值）
  const [pickedModel, setPickedModel] = useState<CatalogModel | undefined>(undefined);
  // 当前「已保存绑定」的限额。换供应商后必须作废：否则手填目录外模型时会写回
  // 上一家供应商的旧限额（端点换成了 B、限额还是 A 的）。
  const [savedLimits, setSavedLimits] = useState(() => ({
    maxTokens: agent.provider?.maxTokens ?? 8192,
    contextWindow: agent.provider?.contextWindow ?? 0,
  }));
  const [saving, setSaving] = useState(false);

  // —— 元信息 / 权限编辑（agent.json 全字段）——
  const [description, setDescription] = useState(agent.description);
  const [subagent, setSubagent] = useState(agent.subagent);
  const [workspace, setWorkspace] = useState(agent.workspace ?? "");
  const [tools, setTools] = useState<string[]>(agent.permissions.tools);
  const [bashMode, setBashMode] = useState<BashMode>(bash.mode);
  const [commands, setCommands] = useState(bash.commands.join("\n"));
  const [sandbox, setSandbox] = useState<SandboxMode>(agent.permissions.sandbox);
  // 自动压缩阈值（窗口占用的百分比，1–100）
  const [compactThreshold, setCompactThreshold] = useState(agent.compactThresholdPercent);
  const [compactTarget, setCompactTarget] = useState(agent.compactTargetPercent?.toString() ?? "");
  useEffect(() => {
    let active = true;
    setTestSession(null);
    setTestLoadError(null);
    void invoke("ensure_test_session", { agentName: agent.name })
      .then((info) => {
        if (!active) return;
        setTestSession(info);
      })
      .catch((errorValue) => {
        if (!active) return;
        const message = formatRuntimeError(errorValue);
        setTestLoadError(message);
        onError(message);
      });
    return () => {
      active = false;
    };
  }, [agent.name, onError, testLoadVersion]);

  const resetTestSession = async () => {
    const info = await invoke("reset_test_session", { agentName: agent.name });
    setTestSession(info);
  };
  // agent 切换时重置编辑状态（否则上一个 Agent 的草稿会串台）
  const agentNameRef = useRef(agent.name);
  if (agentNameRef.current !== agent.name) {
    agentNameRef.current = agent.name;
    setDescription(agent.description);
    setSubagent(agent.subagent);
    setWorkspace(agent.workspace ?? "");
    setTools(agent.permissions.tools);
    setCompactThreshold(agent.compactThresholdPercent);
    setCompactTarget(agent.compactTargetPercent?.toString() ?? "");
    setBashMode(bash.mode);
    setCommands(bash.commands.join("\n"));
    setSandbox(agent.permissions.sandbox);
  }

  const selectedProvider = providers.find((provider) => provider.id === providerId);
  const draftProvider = selectedProvider
    ? {
        id: modelId.trim(),
        name: modelId.trim(),
        api: selectedProvider.api,
        baseUrl: selectedProvider.baseUrl,
        maxTokens: resolveMaxTokens(pickedModel, savedLimits.maxTokens),
        contextWindow: resolveContextWindow(pickedModel, savedLimits.contextWindow),
      }
    : null;
  const boundProviderId = providers.find(
    (provider) => agent.provider
      && provider.api === agent.provider.api
      && provider.baseUrl === agent.provider.baseUrl,
  )?.id ?? "";

  const providerDirty =
    providerId !== boundProviderId
    || modelId.trim() !== (agent.provider?.id ?? "")
    || (draftProvider?.maxTokens ?? 8192) !== (agent.provider?.maxTokens ?? 8192)
    || (draftProvider?.contextWindow ?? 0) !== (agent.provider?.contextWindow ?? 0);

  const saveConfiguration = async (running: boolean) => {
    if (saving || running || tools.length === 0) return;
    if (providerId && !modelId.trim()) {
      onError("请选择或填写默认模型后再保存");
      return;
    }
    const targetPercent = compactTarget.trim() === "" ? null : Number(compactTarget);
    if (targetPercent !== null && (!Number.isInteger(targetPercent) || targetPercent < 1 || targetPercent > 99)) {
      onError("压缩后目标比例须为 1–99 的整数，或留空使用默认行为");
      return;
    }
    setSaving(true);
    try {
      const next: AgentDefinition = {
        ...agent,
        model: modelId.trim(),
        provider: draftProvider,
        description: description.trim(),
        subagent,
        workspace: workspace.trim() || null,
        permissions: permissionsFromFields(tools, bashMode, commands, sandbox),
        // 越界值在核心侧也会兜底，但先在这里夹一次，免得写进文件的是脏值
        compactThresholdPercent: Math.min(100, Math.max(1, Math.round(compactThreshold))),
        compactTargetPercent: targetPercent,
      };
      await invoke("save_agent", { def: next });
      setSavedLimits({
        maxTokens: next.provider?.maxTokens ?? 8192,
        contextWindow: next.provider?.contextWindow ?? 0,
      });
      await onSaved(next.subagent);
      await resetTestSession();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setSaving(false);
    }
  };

  const metaDirty =
    description !== agent.description ||
    subagent !== agent.subagent ||
    (workspace.trim() || null) !== (agent.workspace ?? null) ||
    tools.join(",") !== agent.permissions.tools.join(",") ||
    bashMode !== bash.mode ||
    (bashMode === "allowAll" ? "" : commands) !== bash.commands.join("\n") ||
    sandbox !== agent.permissions.sandbox ||
    Math.min(100, Math.max(1, Math.round(compactThreshold))) !== agent.compactThresholdPercent ||
    (compactTarget.trim() === "" ? null : Number(compactTarget)) !== (agent.compactTargetPercent ?? null);
  const configurationDirty = providerDirty || metaDirty;

  if (!testSession) {
    return (
      <div className="screen agent-workbench-loading">
        <div className="screen-bar">
          <button type="button" className="icon-btn" title="返回" aria-label="返回" onClick={onBack}>
            <IconBack />
          </button>
          <span className="crumb">Agent 工作台 · <b>{agent.name}</b></span>
        </div>
        <div className="empty compact-empty">
          <h2>{testLoadError ? "临时测试初始化失败" : "正在准备临时测试…"}</h2>
          {testLoadError && <p>{testLoadError}</p>}
          {testLoadError && (
            <button type="button" className="btn primary" onClick={() => setTestLoadVersion((value) => value + 1)}>
              重试
            </button>
          )}
        </div>
      </div>
    );
  }

  return (
    <ChatView
      key={`${agent.name}-${testSession.sessionId}`}
      agent={agent}
      providers={providers}
      sessionId={testSession.sessionId}
      sessions={sessions}
      blockedSessionIds={blockedSessionIds}
      onBack={onBack}
      onError={onError}
      onNewSession={async () => false}
      onRunningChange={onRunningChange}
      onSessionReset={(info) => {
        if (info?.temporary) setTestSession(info);
      }}
      mode="test"
      onClearTest={resetTestSession}
      onInspectDetail={() => setSettingsTab("test")}
      renderInspector={(context) => (
        <div className="agent-settings-panel">
          <div className="agent-settings-head">
            <div className="agent-settings-title">
              <span className="mono">{agent.name}</span>
              {configurationDirty && <span className="dirty-badge">未保存</span>}
            </div>
            <div className="sub">{agent.description || "（暂无描述）"}</div>
          </div>

          <div className="agent-settings-tabs" role="tablist" aria-label="Agent 设置">
            {([
              ["basic", "基础"],
              ["permissions", "权限"],
              ["files", "文件"],
              ["test", "测试"],
            ] as const).map(([tab, label]) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={settingsTab === tab}
                className={settingsTab === tab ? "active" : ""}
                onClick={() => setSettingsTab(tab)}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="agent-settings-body">
            {settingsTab === "basic" && (
              <section className="settings-section">
                <div className="settings-field">
                  <label htmlFor="agent-provider-select">provider</label>
                  <ChoiceSelect
                    id="agent-provider-select"
                    value={providerId}
                    choices={providers.map((provider) => ({ value: provider.id, label: provider.name }))}
                    placeholder="（未绑定提供商）"
                    isClearable
                    menuInPortal
                    onChange={(next) => {
                      setProviderId(next);
                      setModelId("");
                      setPickedModel(undefined);
                      setSavedLimits({ maxTokens: 8192, contextWindow: 0 });
                    }}
                  />
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-default-model">default_model</label>
                  <ModelPicker
                    id="agent-default-model"
                    providers={providers}
                    providerId={providerId}
                    modelId={modelId}
                    disabled={saving}
                    placeholder="模型 ID，如 claude-sonnet-4-5"
                    onModelChange={(nextId, model) => {
                      setModelId(nextId);
                      setPickedModel(model);
                    }}
                  />
                  <span className="hint">保存后，新的临时测试和正式会话会使用此模型。</span>
                  {draftProvider && (
                    <span className="hint mono">{draftProvider.api} · {draftProvider.baseUrl}</span>
                  )}
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-description">description</label>
                  <textarea
                    id="agent-description"
                    className="meta-editor"
                    value={description}
                    onChange={(event) => setDescription(event.target.value)}
                    placeholder="这个 Agent 是做什么的？"
                    rows={3}
                  />
                </div>
                <div className="settings-field">
                  <span className="field-label">subagent</span>
                  <label className="ios-toggle">
                    <input
                      type="checkbox"
                      checked={subagent}
                      disabled={saving}
                      onChange={(event) => setSubagent(event.target.checked)}
                    />
                    <span className="slider" />
                    <span className="toggle-label">{subagent ? "归入 SUBS" : "归入 AGENTS"}</span>
                  </label>
                  <span className="hint">由 Agent 的 create_agent 工具创建时默认开启；可在此更改分组。</span>
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-workspace">workspace</label>
                  <input
                    id="agent-workspace"
                    className="mono"
                    value={workspace}
                    onChange={(event) => setWorkspace(event.target.value)}
                    placeholder={`~/.pipi/agents/${agent.name}/workspace（默认）`}
                  />
                </div>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="agent-compact-threshold">compact_threshold</label>
                  <div className="inline-value">
                    <input
                      id="agent-compact-threshold"
                      className="mono"
                      type="number"
                      min={1}
                      max={100}
                      value={compactThreshold}
                      onChange={(event) => setCompactThreshold(Number(event.target.value))}
                    />
                    <span>%</span>
                  </div>
                  <span className="hint">上下文占用达到模型窗口的该比例时自动压缩。</span>
                </div>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="agent-compact-target">压缩后目标比例</label>
                  <div className="inline-value">
                    <input
                      id="agent-compact-target"
                      className="mono"
                      type="number"
                      min={1}
                      max={99}
                      step={1}
                      value={compactTarget}
                      onChange={(event) => setCompactTarget(event.target.value)}
                      placeholder="默认"
                    />
                    <span>%</span>
                  </div>
                  <span className="hint">相对本次压缩前的上下文，目标包含摘要；留空沿用近期 2 万 token 的原文预算。实际结果受完整轮次边界影响。</span>
                </div>
              </section>
            )}

            {settingsTab === "permissions" && (
              <section className="settings-section">
                <div className="settings-field">
                  <span className="field-label">tools</span>
                  <ToolChoices value={tools} className="tool-row settings-choice-grid" onToggle={tool => setTools(previous => previous.includes(tool) ? previous.filter(candidate => candidate !== tool) : [...previous, tool])} />
                  {tools.length === 0 && <span className="field-error">至少保留一个工具。</span>}
                </div>
                <div className="settings-field">
                  <span className="field-label">bash.mode</span>
                  <BashChoices value={bashMode} onChange={setBashMode} name="agent-bash-mode" className="tool-row vertical-choices" />
                  {bashMode !== "allowAll" && (
                    <div className="perm-editor">
                      <textarea
                        value={commands}
                        onChange={(event) => setCommands(event.target.value)}
                        placeholder={bashMode === "allowlist" ? "git\nnpm run\nls" : "rm\nsudo"}
                        rows={5}
                      />
                      <div className="hint">
                        {bashMode === "allowlist"
                          ? "每行一条；单词条目匹配命令名，带空格的条目按前缀匹配。"
                          : "每行一条；命中任意条目的命令将被拒绝。"}
                      </div>
                    </div>
                  )}
                </div>
                <div className="settings-field">
                  <span className="field-label">sandbox</span>
                  <SandboxChoices value={sandbox} onChange={setSandbox} name="agent-sandbox" className="tool-row vertical-choices" />
                  <span className="hint">
                    临时测试会按保存后的真实权限执行；工具造成的工作区改动不会回滚。
                  </span>
                </div>
              </section>
            )}

            {settingsTab === "files" && (
              <section className="settings-section settings-files">
                <div className="settings-summary-row">
                  <span className="field-label">agent_dir</span>
                  <span className="mono">~/.pipi/agents/{agent.name}/</span>
                </div>
                <div className="settings-summary-row">
                  <span className="field-label">mcp_servers</span>
                  <div className="settings-tags">
                    {agent.mcpServers.length
                      ? agent.mcpServers.map((server) => <span className="tag" key={server.name}>{server.name}</span>)
                      : <span className="dim">未配置</span>}
                  </div>
                </div>
                <AgentFileEditor
                  agentName={agent.name}
                  onError={onError}
                  disabled={!context.ready || context.running}
                  onSaved={async () => {
                    await onSaved();
                    await resetTestSession();
                  }}
                />
              </section>
            )}

            {settingsTab === "test" && <SessionDiagnostics context={context} />}
          </div>

          {(settingsTab === "basic" || settingsTab === "permissions") && (
            <div className="agent-settings-footer">
              <div className="save-state">
                {context.running
                  ? "测试运行中，请先停止"
                  : configurationDirty ? "有未保存的 Agent 配置" : "配置已保存"}
              </div>
              <button
                type="button"
                className="btn primary"
                disabled={saving || context.running || !context.ready || !configurationDirty || tools.length === 0}
                onClick={() => void saveConfiguration(context.running)}
              >
                {saving ? "保存中…" : "保存配置"}
              </button>
            </div>
          )}
        </div>
      )}
    />
  );
}
