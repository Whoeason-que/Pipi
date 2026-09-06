import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ChatView from "./Chat";
import {
  API_LABELS,
  SANDBOX_LABELS,
  type AgentDefinition,
  type Theme,
  type ApiKind,
  type BashMode,
  type PermissionsConfig,
  type ProviderConfig,
  type SandboxMode,
  type Settings,
} from "./types";

const BASH_MODE_LABELS: Record<BashMode, string> = {
  allowAll: "全部允许",
  allowlist: "白名单",
  denylist: "黑名单",
};

const KNOWN_TOOLS = ["read", "write", "edit", "bash", "memory"] as const;

function applyTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("pipi-theme", theme);
}

function keyStatus(p: ProviderConfig): { label: string; warn: boolean } {
  if (p.envKey) return { label: `env: ${p.envKey}`, warn: false };
  if (p.apiKey) return { label: "已存密钥", warn: false };
  return { label: "未配置密钥", warn: true };
}

export default function App() {
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [chatOpen, setChatOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setAgents(await invoke<AgentDefinition[]>("list_agents"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
    invoke<Settings>("get_settings")
      .then((s) => {
        setSettings(s);
        applyTheme(s.theme);
      })
      .catch((e) => setError(String(e)));
  }, [refresh]);

  const updateSettings = async (next: Settings) => {
    setSettings(next);
    applyTheme(next.theme);
    try {
      await invoke("save_settings", { settings: next });
    } catch (e) {
      setError(String(e));
    }
  };

  const current = agents.find((a) => a.name === selected) ?? null;

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="sidebar-header">
          <span className="logo">
            <span className="pi">π</span> pipi
          </span>
          <button
            className="icon-btn"
            title="设置"
            onClick={() => setSettingsOpen(true)}
          >
            ⚙
          </button>
        </div>
        <div className="sidebar-section">Agents</div>
        <nav className="agent-list">
          {agents.map((a) => (
            <div
              key={a.name}
              className={`agent-item ${selected === a.name && !creating ? "active" : ""}`}
              onClick={() => {
                setSelected(a.name);
                setCreating(false);
              }}
            >
              <div className="name">{a.name}</div>
              {a.description && <div className="desc">{a.description}</div>}
            </div>
          ))}
        </nav>
        <div className="sidebar-footer">
          <button className="primary wide" onClick={() => setCreating(true)}>
            ＋ 新建 Agent
          </button>
        </div>
      </aside>

      <main className="main">
        {error && <div className="error">{error}</div>}
        {creating ? (
          <CreateForm
            onCreated={async (name) => {
              setCreating(false);
              await refresh();
              setSelected(name);
            }}
            onCancel={() => setCreating(false)}
            onError={setError}
          />
        ) : current ? (
          settings &&
          (chatOpen ? (
            <ChatView
              key={current.name}
              agent={current}
              onBack={() => setChatOpen(false)}
              onError={setError}
            />
          ) : (
            <AgentDetail
              key={current.name}
              agent={current}
              providers={settings.providers}
              onSaved={refresh}
              onChat={() => setChatOpen(true)}
              onError={setError}
            />
          ))
        ) : (
          <EmptyState hasAgents={agents.length > 0} />
        )}
      </main>

      {settingsOpen && settings && (
        <SettingsModal
          settings={settings}
          onChange={updateSettings}
          onClose={() => setSettingsOpen(false)}
        />
      )}
    </div>
  );
}

function EmptyState({ hasAgents }: { hasAgents: boolean }) {
  return (
    <div className="empty">
      <div className="glyph">π</div>
      <h1>{hasAgents ? "选择一个 Agent" : "创建你的第一个 Agent"}</h1>
      <p>
        在 Pipi 里，你维护的不是一条条会话，而是一群有名字、有工作目录、
        有技能和记忆的 Agent。会话只是 Agent 的一次运行记录，是副产品。
      </p>
    </div>
  );
}

// ============ Agent 详情 ============

interface AgentDetailProps {
  agent: AgentDefinition;
  providers: ProviderConfig[];
  onSaved: () => void | Promise<void>;
  onChat: () => void;
  onError: (msg: string) => void;
}

function AgentDetail({ agent, providers, onSaved, onChat, onError }: AgentDetailProps) {
  const { bash } = agent.permissions;
  const [providerId, setProviderId] = useState<string>(() => {
    const bound = providers.find(
      (p) => agent.provider && p.api === agent.provider.api && p.baseUrl === agent.provider.baseUrl,
    );
    return bound?.id ?? "";
  });
  const [modelId, setModelId] = useState(agent.provider?.id ?? "");
  const [saving, setSaving] = useState(false);

  const bindProvider = async () => {
    if (saving) return;
    setSaving(true);
    try {
      const p = providers.find((x) => x.id === providerId);
      const next: AgentDefinition = {
        ...agent,
        model: modelId.trim(),
        provider: p
          ? {
              id: modelId.trim(),
              name: modelId.trim(),
              api: p.api,
              baseUrl: p.baseUrl,
              maxTokens: agent.provider?.maxTokens ?? 8192,
              contextWindow: agent.provider?.contextWindow ?? 0,
            }
          : null,
      };
      await invoke("save_agent", { def: next });
      await onSaved();
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="detail">
      <div className="detail-inner">
        <div className="detail-head">
          <div>
            <h2>
              {agent.name}
              {agent.provider && <span className="badge">{agent.provider.id}</span>}
            </h2>
            <div className="sub">{agent.description || "（暂无描述）"}</div>
          </div>
          <button className="primary" onClick={onChat}>
            ▶ 开始对话
          </button>
        </div>

        <div className="field-grid">
          <div className="field">
            <div className="label">模型</div>
          </div>
          <div className="field">
            <div className="value">
              <div className="bind-row">
                <select
                  value={providerId}
                  onChange={(e) => setProviderId(e.target.value)}
                >
                  <option value="">（未绑定提供商）</option>
                  {providers.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </select>
                <input
                  className="mono"
                  value={modelId}
                  onChange={(e) => setModelId(e.target.value)}
                  placeholder="模型 ID，如 claude-sonnet-4-5"
                />
                <button
                  className="primary"
                  disabled={saving || (!!providerId && !modelId.trim())}
                  onClick={bindProvider}
                >
                  保存
                </button>
              </div>
              {agent.provider && (
                <div className="hint mono">
                  {agent.provider.api} · {agent.provider.baseUrl}
                </div>
              )}
            </div>
          </div>

          <div className="field">
            <div className="label">工作目录</div>
          </div>
          <div className="field">
            <div className="value mono">
              {agent.workspace ?? `~/.pipi/agents/${agent.name}/workspace（默认）`}
            </div>
          </div>

          <div className="field">
            <div className="label">工具</div>
          </div>
          <div className="field">
            <div className="value">
              {agent.permissions.tools.length
                ? agent.permissions.tools.join(" · ")
                : "（无）"}
            </div>
          </div>

          <div className="field">
            <div className="label">命令权限</div>
          </div>
          <div className="field">
            <div className="value">
              {BASH_MODE_LABELS[bash.mode]}
              {bash.mode !== "allowAll" && bash.commands.length > 0 && (
                <div className="mono perm-list">{bash.commands.join("\n")}</div>
              )}
            </div>
          </div>

          <div className="field">
            <div className="label">沙箱</div>
          </div>
          <div className="field">
            <div className="value">
              <span className="badge neutral">{agent.permissions.sandbox}</span>{" "}
              <span style={{ color: "var(--muted)" }}>
                {SANDBOX_LABELS[agent.permissions.sandbox]}
              </span>
            </div>
          </div>

          <div className="field">
            <div className="label">MCP 服务器</div>
          </div>
          <div className="field">
            <div className={`value ${agent.mcpServers.length ? "" : "dim"}`}>
              {agent.mcpServers.length
                ? agent.mcpServers.map((m) => m.name).join("、")
                : "未配置（M3 支持）"}
            </div>
          </div>

          <div className="field">
            <div className="label">文件</div>
          </div>
          <div className="field">
            <div className="value dim mono">
              一切皆文件 —— ~/.pipi/agents/{agent.name}/ 下的 agent.json、
              AGENTS.md、skills/、memory/ 直接编辑即生效
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// ============ 新建 Agent ============

interface CreateFormProps {
  onCreated: (name: string) => void | Promise<void>;
  onCancel: () => void;
  onError: (msg: string) => void;
}

function CreateForm({ onCreated, onCancel, onError }: CreateFormProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [tools, setTools] = useState<string[]>([...KNOWN_TOOLS]);
  const [bashMode, setBashMode] = useState<BashMode>("allowAll");
  const [commands, setCommands] = useState("");
  const [sandbox, setSandbox] = useState<SandboxMode>("workspace-write");
  const [submitting, setSubmitting] = useState(false);

  const toggleTool = (tool: string) => {
    setTools((prev) =>
      prev.includes(tool) ? prev.filter((t) => t !== tool) : [...prev, tool],
    );
  };

  const submit = async () => {
    if (!name.trim() || submitting) return;
    setSubmitting(true);
    onError(null as unknown as string);
    try {
      const permissions: PermissionsConfig = {
        tools,
        bash: {
          mode: bashMode,
          commands:
            bashMode === "allowAll"
              ? []
              : commands
                  .split("\n")
                  .map((c) => c.trim())
                  .filter(Boolean),
        },
        sandbox,
      };
      await invoke("create_agent", {
        name,
        description,
        workspace: workspace.trim() || null,
        permissions,
      });
      await onCreated(name.trim());
    } catch (e) {
      onError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="detail">
      <div className="detail-inner">
        <div className="form">
          <h2>新建 Agent</h2>
          <div className="sub">
            一切皆文件：将在 <span className="mono">~/.pipi/agents/&lt;name&gt;/</span>{" "}
            下生成 <span className="mono">agent.json</span>、
            <span className="mono">AGENTS.md</span>、
            <span className="mono">skills/</span>、<span className="mono">memory/</span>
            、<span className="mono">sessions/</span>
          </div>

          <div className="field">
            <label className="label">名称</label>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="例如：code-reviewer"
              autoFocus
            />
            <div className="hint">仅限字母、数字、- 和 _，将作为目录名</div>
          </div>

          <div className="field">
            <label className="label">描述</label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="这个 Agent 是做什么的？"
            />
          </div>

          <div className="field">
            <label className="label">工作目录</label>
            <input
              className="mono"
              value={workspace}
              onChange={(e) => setWorkspace(e.target.value)}
              placeholder="例如：~/projects/my-app"
            />
            <div className="hint">
              Agent 只在此目录内工作；留空使用默认的 agent 目录内 workspace/
            </div>
          </div>

          <div className="field">
            <label className="label">工具</label>
            <div className="tool-row">
              {KNOWN_TOOLS.map((tool) => (
                <label key={tool} className="tool-check">
                  <input
                    type="checkbox"
                    checked={tools.includes(tool)}
                    onChange={() => toggleTool(tool)}
                  />
                  <span className="mono">{tool}</span>
                </label>
              ))}
            </div>
          </div>

          <div className="field">
            <label className="label">沙箱</label>
            <div className="tool-row">
              {(Object.keys(SANDBOX_LABELS) as SandboxMode[]).map((mode) => (
                <label key={mode} className="tool-check">
                  <input
                    type="radio"
                    name="sandbox"
                    checked={sandbox === mode}
                    onChange={() => setSandbox(mode)}
                  />
                  <span>{SANDBOX_LABELS[mode]}</span>
                </label>
              ))}
            </div>
            <div className="hint">
              只读：不执行命令、不写文件；工作目录内可写：强制删除类命令与越出
              工作目录的写入被拒绝；完全访问：不设限
            </div>
          </div>

          <div className="field">
            <label className="label">命令权限（bash）</label>
            <div className="tool-row">
              {(Object.keys(BASH_MODE_LABELS) as BashMode[]).map((mode) => (
                <label key={mode} className="tool-check">
                  <input
                    type="radio"
                    name="bash-mode"
                    checked={bashMode === mode}
                    onChange={() => setBashMode(mode)}
                  />
                  <span>{BASH_MODE_LABELS[mode]}</span>
                </label>
              ))}
            </div>
            {bashMode !== "allowAll" && (
              <div className="perm-editor">
                <textarea
                  value={commands}
                  onChange={(e) => setCommands(e.target.value)}
                  placeholder={bashMode === "allowlist" ? "git\nnpm run\nls" : "rm\nsudo"}
                />
                <div className="hint">
                  {bashMode === "allowlist"
                    ? "每行一条；单词条目匹配以该词开头的命令，带空格按前缀匹配"
                    : "每行一条；命中任意条目的命令将被拒绝"}
                </div>
              </div>
            )}
          </div>

          <div className="actions">
            <button className="ghost" onClick={onCancel}>
              取消
            </button>
            <button
              className="primary"
              disabled={!name.trim() || tools.length === 0 || submitting}
              onClick={submit}
            >
              创建
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ============ 设置弹窗 ============

interface SettingsModalProps {
  settings: Settings;
  onChange: (next: Settings) => void | Promise<void>;
  onClose: () => void;
}

function SettingsModal({ settings, onChange, onClose }: SettingsModalProps) {
  const [editing, setEditing] = useState<ProviderConfig | "new" | null>(null);

  const saveProvider = (p: ProviderConfig) => {
    const providers = [...settings.providers];
    const idx = providers.findIndex((x) => x.id === p.id);
    if (idx >= 0) providers[idx] = p;
    else providers.push(p);
    onChange({ ...settings, providers });
    setEditing(null);
  };

  const deleteProvider = (id: string) => {
    if (!confirm(`删除提供商「${id}」？（已绑定它的 Agent 不受影响，但需重新配置）`)) return;
    onChange({
      ...settings,
      providers: settings.providers.filter((p) => p.id !== id),
      defaultProviderId:
        settings.defaultProviderId === id ? null : settings.defaultProviderId,
    });
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h2>设置</h2>
          <button className="icon-btn" onClick={onClose} title="关闭">
            ✕
          </button>
        </div>

        <div className="modal-section">
          <span className="label">主题</span>
          <div className="theme-row">
            <ThemeOption
              active={settings.theme === "dark"}
              name="深色"
              swatch={["#111111", "#181818", "#0169CC", "#FCFCFC"]}
              onClick={() => onChange({ ...settings, theme: "dark" })}
            />
            <ThemeOption
              active={settings.theme === "light"}
              name="浅色"
              swatch={["#FCFCFC", "#FFFFFF", "#0169CC", "#111111"]}
              onClick={() => onChange({ ...settings, theme: "light" })}
            />
          </div>
        </div>

        <div className="modal-section">
          <span className="label">模型提供商</span>
          {settings.providers.map((p) => {
            const status = keyStatus(p);
            return (
              <div className="provider-row" key={p.id}>
                <div className="info">
                  <div className="p-name">
                    {p.name}
                    <span className="badge">{API_LABELS[p.api]}</span>
                    <span className={`badge ${status.warn ? "warn" : "neutral"}`}>
                      {status.label}
                    </span>
                    {settings.defaultProviderId === p.id && (
                      <span className="badge">默认</span>
                    )}
                  </div>
                  <div className="p-url mono">{p.baseUrl}</div>
                </div>
                <div className="p-actions">
                  <button className="link" onClick={() => setEditing(p)}>
                    编辑
                  </button>
                  {settings.defaultProviderId !== p.id && (
                    <button
                      className="link"
                      onClick={() =>
                        onChange({ ...settings, defaultProviderId: p.id })
                      }
                    >
                      设为默认
                    </button>
                  )}
                  <button className="link danger" onClick={() => deleteProvider(p.id)}>
                    删除
                  </button>
                </div>
              </div>
            );
          })}

          {editing === null && (
            <button className="ghost" onClick={() => setEditing("new")}>
              ＋ 添加提供商
            </button>
          )}

          {editing !== null && (
            <ProviderForm
              initial={editing === "new" ? null : editing}
              existingIds={settings.providers.map((p) => p.id)}
              onSave={saveProvider}
              onCancel={() => setEditing(null)}
            />
          )}
        </div>
      </div>
    </div>
  );
}

function ThemeOption({
  active,
  name,
  swatch,
  onClick,
}: {
  active: boolean;
  name: string;
  swatch: string[];
  onClick: () => void;
}) {
  return (
    <div className={`theme-option ${active ? "active" : ""}`} onClick={onClick}>
      <div className="swatch">
        {swatch.map((c) => (
          <span key={c} style={{ background: c }} />
        ))}
      </div>
      <div className="name">
        {name}
        {active && <span style={{ color: "var(--accent-text)" }}> ✓</span>}
      </div>
    </div>
  );
}

interface ProviderFormProps {
  initial: ProviderConfig | null;
  existingIds: string[];
  onSave: (p: ProviderConfig) => void;
  onCancel: () => void;
}

function ProviderForm({ initial, existingIds, onSave, onCancel }: ProviderFormProps) {
  const [name, setName] = useState(initial?.name ?? "");
  const [api, setApi] = useState<ApiKind>(initial?.api ?? "anthropic-messages");
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? "");
  const [envKey, setEnvKey] = useState(initial?.envKey ?? "");
  const [apiKey, setApiKey] = useState(initial?.apiKey ?? "");

  const id = initial?.id ?? slugify(name);

  const valid =
    name.trim().length > 0 &&
    baseUrl.trim().startsWith("http") &&
    (initial !== null || !existingIds.includes(id));

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
      <div className="actions">
        <button className="ghost" onClick={onCancel}>
          取消
        </button>
        <button className="primary" disabled={!valid} onClick={submit}>
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
  return slug || `provider-${Date.now()}`;
}
