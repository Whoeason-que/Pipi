import { useEffect, useState } from "react";
import { resolveContextWindow, resolveMaxTokens } from "../catalog";
import ChatView from "../chat/ChatView";
import { formatRuntimeError } from "../chat-runtime";
import Inspector from "../chat/Inspector";
import { useI18n } from "../i18n";
import { IconBack } from "../icons";
import { ModelPicker } from "../ModelPicker";
import { ReasoningSelect } from "../ReasoningSelect";
import { invoke } from "../platform";
import { ChoiceSelect } from "../Select";
import type { SessionRuntime } from "../session-runtime";
import {
  type AgentDefinition,
  type McpServerStatusView,
  type PromptScope,
  type ProviderConfig,
  type SessionInfoView,
} from "../types";
import { permissionsFromFields } from "./agent-fields";
import {
  PromptScopeChoices,
  SandboxChoices,
  ToolChoices,
} from "./PermissionChoices";
import { useAgentConfigFields } from "./use-agent-config-fields";

import AgentFileEditor from "./AgentFileEditor";
import AgentAvatar from "../components/AgentAvatar";
import { AgentSkillsTab } from "../skills/AgentSkillsTab";
// ============ Agent 详情 ============

interface AgentWorkbenchProps {
  agent: AgentDefinition;
  providers: ProviderConfig[];
  onSaved: (subagent?: boolean) => void | Promise<void>;
  onBack: () => void;
  onOpenFile?: (path: string) => void;
  onError: (msg: string) => void;
  blockedSessionIds: string[];
  sessions: SessionRuntime;
  onRunningChange: () => void;
}

type AgentSettingsTab = "basic" | "permissions" | "skills" | "files" | "test";

export default function AgentWorkbench({
  agent,
  providers,
  onSaved,
  onBack,
  onOpenFile,
  onError,
  blockedSessionIds,
  sessions,
  onRunningChange,
}: AgentWorkbenchProps) {
  const { t } = useI18n();
  const [testSession, setTestSession] = useState<SessionInfoView | null>(null);
  const [testLoadError, setTestLoadError] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [importReport, setImportReport] = useState<string | null>(null);
  const [testLoadVersion, setTestLoadVersion] = useState(0);
  const [settingsTab, setSettingsTab] = useState<AgentSettingsTab>("basic");
  // —— 共享字段（供应商 / 模型 / 描述 / 工作目录 / 工具 / 沙箱）——
  // 状态与「换供应商清模型」联动在 useAgentConfigFields；工作台只保留自己的
  // savedLimits、subagent、压缩阈值、promptScope 与保存/校验逻辑。
  const {
    providerId,
    modelId,
    reasoningEffort,
    setReasoningEffort,
    pickedModel,
    description,
    workspace,
    tools,
    sandbox,
    setDescription,
    setWorkspace,
    setTools,
    setSandbox,
    changeProvider,
    changeModel,
    toggleTool,
  } = useAgentConfigFields({
    providerId:
      providers.find(
        (p) =>
          agent.provider &&
          p.api === agent.provider.api &&
          p.baseUrl === agent.provider.baseUrl,
      )?.id ?? "",
    modelId: agent.provider?.id ?? "",
    reasoningEffort: agent.provider?.reasoningEffort,
    description: agent.description,
    workspace: agent.workspace ?? "",
    tools: agent.permissions.tools,
    sandbox: agent.permissions.sandbox,
  });
  // 当前「已保存绑定」的限额。换供应商后必须作废：否则手填目录外模型时会写回
  // 上一家供应商的旧限额（端点换成了 B、限额还是 A 的）。
  const [savedLimits, setSavedLimits] = useState(() => ({
    maxTokens: agent.provider?.maxTokens ?? 8192,
    contextWindow: agent.provider?.contextWindow ?? 0,
  }));
  const [saving, setSaving] = useState(false);

  // —— MCP 服务器状态（只读，来自宿主）：切换页签或保存后刷新 ——
  const [mcpStatus, setMcpStatus] = useState<McpServerStatusView[]>([]);
  useEffect(() => {
    if (!agent.mcpServers.length) {
      setMcpStatus([]);
      return;
    }
    let cancelled = false;
    invoke("mcp_status", { agentName: agent.name })
      .then((statuses) => {
        if (!cancelled) setMcpStatus(statuses);
      })
      .catch(() => {
        // 状态查询失败不影响配置编辑，按「未连接」展示。
        if (!cancelled) setMcpStatus([]);
      });
    return () => {
      cancelled = true;
    };
  }, [agent.name, agent.mcpServers.length, settingsTab, saving]);

  // —— 元信息 / 权限编辑（agent.json 全字段）——
  const [subagent, setSubagent] = useState(agent.subagent);
  // 自动压缩阈值（窗口占用的百分比，1–100）
  const [compactThreshold, setCompactThreshold] = useState(
    agent.compactThresholdPercent,
  );
  const [compactTarget, setCompactTarget] = useState(
    agent.compactTargetPercent?.toString() ?? "",
  );
  const [promptScope, setPromptScope] = useState<PromptScope>(
    agent.systemPrompt,
  );
  useEffect(() => {
    let active = true;
    setTestSession(null);
    setTestLoadError(null);
    void invoke("open_workbench_session", { agentName: agent.name })
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

  /** 把旧 JSONL 会话导入 git：逐条校验、幂等，原文件改名保留。 */
  const importLegacySessions = async () => {
    setImporting(true);
    try {
      const report = await invoke("import_sessions", { agentName: agent.name });
      const failed = report.failed.length;
      setImportReport(
        failed
          ? t("agents.importReportFailed", {
              imported: report.imported.length,
              skipped: report.skipped.length,
              failed,
            })
          : t("agents.importReport", {
              imported: report.imported.length,
              skipped: report.skipped.length,
            }),
      );
      if (failed) {
        onError(
          t("agents.importFailed", {
            details: report.failed
              .map((item) => `${item.sessionId} (${item.reason})`)
              .join("; "),
          }),
        );
      }
      await onSaved();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setImporting(false);
    }
  };

  const resetTestSession = async () => {
    const info = await invoke("open_workbench_session", {
      agentName: agent.name,
      reset: true,
    });
    if (testSession) sessions.forget(agent.name, testSession.sessionId);
    setTestSession(info);
  };

  const selectedProvider = providers.find(
    (provider) => provider.id === providerId,
  );
  const draftProvider = selectedProvider
    ? {
        id: modelId.trim(),
        name: modelId.trim(),
        api: selectedProvider.api,
        baseUrl: selectedProvider.baseUrl,
        maxTokens: resolveMaxTokens(pickedModel, savedLimits.maxTokens),
        contextWindow: resolveContextWindow(
          pickedModel,
          savedLimits.contextWindow,
        ),
        reasoningEffort,
      }
    : null;
  const boundProviderId =
    providers.find(
      (provider) =>
        agent.provider &&
        provider.api === agent.provider.api &&
        provider.baseUrl === agent.provider.baseUrl,
    )?.id ?? "";

  const providerDirty =
    providerId !== boundProviderId ||
    modelId.trim() !== (agent.provider?.id ?? "") ||
    (draftProvider?.maxTokens ?? 8192) !==
      (agent.provider?.maxTokens ?? 8192) ||
    (draftProvider?.contextWindow ?? 0) !==
      (agent.provider?.contextWindow ?? 0) ||
    (draftProvider?.reasoningEffort ?? null) !==
      (agent.provider?.reasoningEffort ?? null);

  const saveConfiguration = async (running: boolean) => {
    if (saving || running || tools.length === 0) return;
    if (providerId && !modelId.trim()) {
      onError(t("agents.selectModelFirst"));
      return;
    }
    const targetPercent =
      compactTarget.trim() === "" ? null : Number(compactTarget);
    if (
      targetPercent !== null &&
      (!Number.isInteger(targetPercent) ||
        targetPercent < 1 ||
        targetPercent > 99)
    ) {
      onError(t("agents.invalidCompactTarget"));
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
        permissions: permissionsFromFields(tools, sandbox),
        // 越界值在核心侧也会兜底，但先在这里夹一次，免得写进文件的是脏值
        compactThresholdPercent: Math.min(
          100,
          Math.max(1, Math.round(compactThreshold)),
        ),
        compactTargetPercent: targetPercent,
        systemPrompt: promptScope,
      };
      await invoke("save_agent", { def: next });
      // 保存时可能剔除了退役工具（glob/grep）；同步本地状态，避免刚保存完
      // 就因为「字段与文件不一致」亮出未保存徽标。
      setTools(next.permissions.tools);
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
    sandbox !== agent.permissions.sandbox ||
    Math.min(100, Math.max(1, Math.round(compactThreshold))) !==
      agent.compactThresholdPercent ||
    (compactTarget.trim() === "" ? null : Number(compactTarget)) !==
      (agent.compactTargetPercent ?? null) ||
    promptScope !== agent.systemPrompt;
  const configurationDirty = providerDirty || metaDirty;

  if (!testSession) {
    return (
      <div className="screen agent-workbench-loading">
        <div className="screen-bar">
          <button
            type="button"
            className="icon-btn"
            title={t("common.back")}
            aria-label={t("common.back")}
            onClick={onBack}
          >
            <IconBack />
          </button>
          <span className="truncate caption crumb">
            {t("agents.workbenchTitle")} · <b>{agent.name}</b>
          </span>
        </div>
        <div className="empty-state empty compact-empty">
          <h2>
            {testLoadError
              ? t("agents.testInitFailed")
              : t("agents.testPreparing")}
          </h2>
          {testLoadError && <p>{testLoadError}</p>}
          {testLoadError && (
            <button
              type="button"
              className="btn primary"
              onClick={() => setTestLoadVersion((value) => value + 1)}
            >
              {t("common.retry")}
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
            <AgentAvatar name={agent.name} subagent={agent.subagent} />
            <div className="agent-settings-heading">
              <div className="agent-settings-title">
                <span className="mono">{agent.name}</span>
                {configurationDirty && (
                  <span className="dirty-badge">{t("agents.unsaved")}</span>
                )}
              </div>
              <div className="sub">
                {agent.description || t("agents.noDescription")}
              </div>
            </div>
          </div>

          <div
            className="agent-settings-tabs"
            role="tablist"
            aria-label={t("agents.settingsAria")}
          >
            {(
              [
                ["basic", t("agents.tabs.basic")],
                ["permissions", t("agents.tabs.permissions")],
                ["skills", t("nav.skills")],
                ["files", t("agents.tabs.files")],
                ["test", t("agents.tabs.test")],
              ] as const
            ).map(([tab, label]) => (
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
                  <label htmlFor="agent-provider-select">
                    {t("agents.modelProvider")}
                  </label>
                  <ChoiceSelect
                    id="agent-provider-select"
                    value={providerId}
                    choices={providers.map((provider) => ({
                      value: provider.id,
                      label: provider.name,
                    }))}
                    placeholder={t("agents.noProvider")}
                    isClearable
                    menuInPortal
                    onChange={(next) => {
                      changeProvider(next);
                      setSavedLimits({ maxTokens: 8192, contextWindow: 0 });
                    }}
                  />
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-default-model">
                    {t("workspace.defaultModel")}
                  </label>
                  <ModelPicker
                    id="agent-default-model"
                    providers={providers}
                    providerId={providerId}
                    modelId={modelId}
                    disabled={saving}
                    placeholder={t("agents.modelIdPlaceholder")}
                    onModelChange={changeModel}
                  />
                  <span className="hint">{t("agents.modelHint")}</span>
                  {draftProvider && (
                    <span className="hint mono">
                      {draftProvider.api} · {draftProvider.baseUrl}
                    </span>
                  )}
                </div>
                <div className="settings-field">
                  <ReasoningSelect
                    id="agent-reasoning-effort"
                    value={reasoningEffort}
                    onChange={setReasoningEffort}
                    disabled={saving || !providerId || !modelId.trim()}
                  />
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-description">
                    {t("agents.description")}
                  </label>
                  <textarea
                    id="agent-description"
                    className="text-input meta-editor"
                    value={description}
                    onChange={(event) => setDescription(event.target.value)}
                    placeholder={t("agents.descriptionPlaceholder")}
                    rows={3}
                  />
                </div>
                <div className="settings-field">
                  <span className="field-label">{t("agents.group")}</span>
                  <label className="ios-toggle">
                    <input
                      type="checkbox"
                      checked={subagent}
                      disabled={saving}
                      onChange={(event) => setSubagent(event.target.checked)}
                    />
                    <span className="slider" />
                    <span className="toggle-label">
                      {subagent
                        ? t("agents.groupSubagent")
                        : t("agents.groupAgent")}
                    </span>
                  </label>
                  <span className="hint">{t("agents.groupHint")}</span>
                </div>
                <div className="settings-field">
                  <label htmlFor="agent-workspace">
                    {t("workspace.directory")}
                  </label>
                  <input
                    id="agent-workspace"
                    className="text-input mono"
                    value={workspace}
                    onChange={(event) => setWorkspace(event.target.value)}
                    placeholder={t("agents.workspacePlaceholder", {
                      name: agent.name,
                    })}
                  />
                </div>
                <div className="settings-field">
                  <span className="field-label">{t("agents.promptScope")}</span>
                  <PromptScopeChoices
                    value={promptScope}
                    onChange={setPromptScope}
                    name="agent-prompt-scope"
                  />
                  <span className="hint">{t("agents.promptScopeHint")}</span>
                </div>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="agent-compact-threshold">
                    {t("agents.compactThreshold")}
                  </label>
                  <div className="inline-value">
                    <input
                      id="agent-compact-threshold"
                      className="text-input mono"
                      type="number"
                      min={1}
                      max={100}
                      value={compactThreshold}
                      onChange={(event) =>
                        setCompactThreshold(Number(event.target.value))
                      }
                    />
                    <span>%</span>
                  </div>
                  <span className="hint">
                    {t("agents.compactThresholdHint")}
                  </span>
                </div>
                <div className="settings-field compact-threshold-field">
                  <label htmlFor="agent-compact-target">
                    {t("agents.compactTarget")}
                  </label>
                  <div className="inline-value">
                    <input
                      id="agent-compact-target"
                      className="text-input mono"
                      type="number"
                      min={1}
                      max={99}
                      step={1}
                      value={compactTarget}
                      onChange={(event) => setCompactTarget(event.target.value)}
                      placeholder={t("agents.defaultPlaceholder")}
                    />
                    <span>%</span>
                  </div>
                  <span className="hint">{t("agents.compactTargetHint")}</span>
                </div>
              </section>
            )}

            {settingsTab === "permissions" && (
              <section className="settings-section">
                <div className="settings-field">
                  <span className="field-label">{t("agents.availableTools")}</span>
                  <ToolChoices
                    value={tools}
                    className="tool-row settings-choice-grid"
                    onToggle={toggleTool}
                  />
                  {tools.length === 0 && (
                    <span className="field-error">
                      {t("agents.atLeastOneTool")}
                    </span>
                  )}
                </div>
                <div className="settings-field">
                  <span className="field-label">
                    {t("agents.sandboxPermissions")}
                  </span>
                  <SandboxChoices
                    value={sandbox}
                    onChange={setSandbox}
                    name="agent-sandbox"
                    className="tool-row vertical-choices"
                  />
                  <span className="hint">{t("agents.sandboxHint")}</span>
                  {tools.includes("browser") && <span className="hint">{t("agents.browserSandboxHint")}</span>}
                </div>
              </section>
            )}

            {settingsTab === "skills" && (
              <AgentSkillsTab
                agentName={agent.name}
                onError={onError}
                onChanged={() => resetTestSession()}
              />
            )}

            {settingsTab === "files" && (
              <section className="settings-section settings-files">
                <div className="settings-summary-row">
                  <span className="field-label">
                    {t("agents.agentDirectory")}
                  </span>
                  <span className="mono">~/.pipi/agents/{agent.name}/</span>
                </div>
                <div className="settings-summary-row">
                  <span className="field-label">{t("agents.legacySessions")}</span>
                  <div className="settings-tags">
                    <button
                      type="button"
                      className="btn"
                      disabled={importing || context.running}
                      onClick={() => void importLegacySessions()}
                    >
                      {importing
                        ? t("agents.importing")
                        : t("agents.importToGit")}
                    </button>
                    {importReport && (
                      <span className="dim">{importReport}</span>
                    )}
                  </div>
                </div>
                <div className="hint">{t("agents.legacyImportHint")}</div>
                <div className="settings-summary-row">
                  <span className="field-label">{t("agents.mcpServers")}</span>
                  {agent.mcpServers.length ? (
                    <div className="mcp-status-list">
                      {agent.mcpServers.map((server) => {
                        const status = mcpStatus.find(
                          (item) => item.name === server.name,
                        );
                        const connected = status?.connected ?? false;
                        return (
                          <div className="mcp-status-item" key={server.name}>
                            <span
                              className={`mcp-status-dot${connected ? " on" : ""}`}
                              title={
                                connected
                                  ? t("agents.connected")
                                  : t("agents.disconnected")
                              }
                            />
                            <span className="mono">{server.name}</span>
                            <span className="dim">
                              {!server.enabled
                                ? t("agents.disabled")
                                : status
                                  ? t("agents.toolCount", {
                                      count: status.tools,
                                    })
                                  : t("agents.disconnected")}
                            </span>
                            <span className="dim">{t(`workspace.sandbox.${server.sandbox ?? agent.permissions.sandbox}`)}</span>
                            {status?.declaredTools?.length ? (
                              <details className="mcp-status-tools">
                                <summary>{t("agents.showMcpTools")}</summary>
                                <ul>{status.declaredTools.map((tool) => <li className="mono" key={tool}>{tool}</li>)}</ul>
                              </details>
                            ) : null}
                            {status?.error && (
                              <span className="field-error">{status.error}</span>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  ) : (
                    <span className="dim">{t("agents.notConfigured")}</span>
                  )}
                  <div className="hint">{t("agents.mcpHint")}</div>
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

            {settingsTab === "test" && (
              <Inspector
                context={context}
                onCompact={() => {}}
                onOpenFile={onOpenFile}
                embedded
                allowCompact={false}
                temporary
              />
            )}
          </div>

          {(settingsTab === "basic" || settingsTab === "permissions") && (
            <div className="agent-settings-footer">
              <div className="truncate caption save-state">
                {context.running
                  ? t("agents.runningHint")
                  : configurationDirty
                    ? t("agents.unsavedConfig")
                    : t("agents.configSaved")}
              </div>
              <button
                type="button"
                className="btn primary"
                disabled={
                  saving ||
                  context.running ||
                  !context.ready ||
                  !configurationDirty ||
                  tools.length === 0
                }
                onClick={() => void saveConfiguration(context.running)}
              >
                {saving ? t("agents.saving") : t("agents.saveConfig")}
              </button>
            </div>
          )}
        </div>
      )}
    />
  );
}
