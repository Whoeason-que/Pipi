import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// ---- 与 Rust 侧类型对齐（serde camelCase）----

export type BashMode = "allowAll" | "allowlist" | "denylist";

export interface BashPermissions {
  mode: BashMode;
  commands: string[];
}

export interface PermissionsConfig {
  tools: string[];
  bash: BashPermissions;
}

export interface McpServer {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
}

export interface AgentDefinition {
  name: string;
  description: string;
  model: string;
  provider: unknown | null;
  workspace: string | null;
  permissions: PermissionsConfig;
  mcpServers: McpServer[];
}

const KNOWN_TOOLS = ["read", "write", "edit", "bash", "memory"] as const;

const BASH_MODE_LABELS: Record<BashMode, string> = {
  allowAll: "全部允许",
  allowlist: "白名单",
  denylist: "黑名单",
};

export default function App() {
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
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
  }, [refresh]);

  const current = agents.find((a) => a.name === selected) ?? null;

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="sidebar-header">
          <span className="logo">
            <span className="pi">π</span> pipi
          </span>
        </div>
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
          <AgentDetail agent={current} />
        ) : (
          <EmptyState hasAgents={agents.length > 0} />
        )}
      </main>
    </div>
  );
}

function EmptyState({ hasAgents }: { hasAgents: boolean }) {
  return (
    <div className="empty">
      <h1>{hasAgents ? "选择一个 Agent" : "创建你的第一个 Agent"}</h1>
      <p>
        在 Pipi 里，你维护的不是一条条会话，而是一群有名字、有工作目录、
        有技能和记忆的 Agent。会话只是 Agent 的一次运行记录，是副产品。
      </p>
    </div>
  );
}

function AgentDetail({ agent }: { agent: AgentDefinition }) {
  const { bash } = agent.permissions;
  return (
    <div className="detail">
      <h2>{agent.name}</h2>
      <div className="sub">{agent.description || "（暂无描述）"}</div>

      <div className="field">
        <div className="label">模型</div>
        <div className={`value mono ${agent.model ? "" : "dim"}`}>
          {agent.provider
            ? `${(agent.provider as { modelId?: string }).modelId ?? "?"}`
            : agent.model || "未配置（M1 支持）"}
        </div>
      </div>

      <div className="field">
        <div className="label">工作目录</div>
        <div className="value mono">
          {agent.workspace ?? `~/.pipi/agents/${agent.name}/workspace（默认）`}
        </div>
      </div>

      <div className="field">
        <div className="label">工具</div>
        <div className="value">
          {agent.permissions.tools.length
            ? agent.permissions.tools.join(" · ")
            : "（无）"}
        </div>
      </div>

      <div className="field">
        <div className="label">命令权限</div>
        <div className="value">
          {BASH_MODE_LABELS[bash.mode]}
          {bash.mode !== "allowAll" && bash.commands.length > 0 && (
            <div className="mono perm-list">{bash.commands.join("\n")}</div>
          )}
        </div>
      </div>

      <div className="field">
        <div className="label">MCP 服务器</div>
        <div className={`value ${agent.mcpServers.length ? "" : "dim"}`}>
          {agent.mcpServers.length
            ? agent.mcpServers.map((m) => m.name).join("、")
            : "未配置（M3 支持）"}
        </div>
      </div>

      <div className="field">
        <div className="label">文件</div>
        <div className="value mono dim">
          一切皆文件 —— ~/.pipi/agents/{agent.name}/ 下的 agent.json、AGENTS.md、
          skills/、memory/ 直接编辑即生效
        </div>
      </div>
    </div>
  );
}

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
          <div className="label">名称</div>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="例如：code-reviewer"
            autoFocus
          />
          <div className="hint">仅限字母、数字、- 和 _，将作为目录名</div>
        </div>

        <div className="field">
          <div className="label">描述</div>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="这个 Agent 是做什么的？"
          />
        </div>

        <div className="field">
          <div className="label">工作目录</div>
          <input
            value={workspace}
            onChange={(e) => setWorkspace(e.target.value)}
            placeholder="例如：~/projects/my-app"
            className="mono"
          />
          <div className="hint">
            Agent 只在此目录内工作；留空使用默认的 agent 目录内 workspace/
          </div>
        </div>

        <div className="field">
          <div className="label">工具</div>
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
          <div className="label">命令权限（bash）</div>
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
  );
}
