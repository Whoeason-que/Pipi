import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, listen } from "./platform";
import ChatView from "./Chat";
import {
  formatRuntimeError,
  normalizeAgentEvent,
  type AgentEventPayload,
} from "./chat-runtime";
import {
  API_LABELS,
  SANDBOX_LABELS,
  type AgentDefinition,
  type SessionInfoView,
  type SessionSummaryView,
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
  const [chatKey, setChatKey] = useState(0);
  const [sessionsByAgent, setSessionsByAgent] = useState<Record<string, SessionSummaryView[]>>({});
  const [activeSession, setActiveSession] = useState<SessionInfoView | null>(null);
  const [chatRunning, setChatRunning] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const agentsRef = useRef<AgentDefinition[]>([]);
  const sessionListRequestRef = useRef(0);
  const agentRequestRef = useRef(0);
  const navigationRequestRef = useRef(0);
  const sessionInfoRequestRef = useRef(0);
  const settingsSaveRequestRef = useRef(0);
  const settingsRef = useRef<Settings | null>(null);
  const settingsSaveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const selectedRef = useRef<string | null>(null);
  const activeSessionRef = useRef<SessionInfoView | null>(null);
  const blockedSessionIdsRef = useRef(new Set<string>());
  const navigationQueueRef = useRef<Promise<void>>(Promise.resolve());
  agentsRef.current = agents;
  settingsRef.current = settings;
  selectedRef.current = selected;
  activeSessionRef.current = activeSession;

  const invalidateNavigation = () => {
    navigationRequestRef.current += 1;
    sessionInfoRequestRef.current += 1;
  };

  const enqueueNavigation = async (
    requestId: number,
    operation: () => Promise<void>,
  ): Promise<void> => {
    const previous = navigationQueueRef.current;
    let release!: () => void;
    navigationQueueRef.current = new Promise<void>((resolve) => {
      release = resolve;
    });
    await previous;
    try {
      if (requestId === navigationRequestRef.current) await operation();
    } finally {
      release();
    }
  };

  const refreshSessions = useCallback(async (names: string[]) => {
    const requestId = ++sessionListRequestRef.current;
    const results = await Promise.all(
      names.map(async (name) => {
        try {
          return {
            name,
            sessions: await invoke<SessionSummaryView[]>("list_sessions", { agentName: name }),
            failed: false,
          } as const;
        } catch {
          return { name, sessions: null, failed: true } as const;
        }
      }),
    );
    if (requestId !== sessionListRequestRef.current) return;
    setSessionsByAgent((previous) => {
      const next: Record<string, SessionSummaryView[]> = {};
      for (const result of results) {
        if (!result.failed && result.sessions) next[result.name] = result.sessions;
        else if (previous[result.name]) next[result.name] = previous[result.name];
      }
      return next;
    });
    const failedNames = results.filter((result) => result.failed).map((result) => result.name);
    if (failedNames.length > 0) {
      setError(`会话列表加载失败：${failedNames.join("、")}（保留旧数据）`);
    }
  }, []);

  const refresh = useCallback(async () => {
    const requestId = ++agentRequestRef.current;
    try {
      const nextAgents = await invoke<AgentDefinition[]>("list_agents");
      if (requestId !== agentRequestRef.current) return;
      agentsRef.current = nextAgents;
      setAgents(nextAgents);
      setError(null);
    } catch (errorValue) {
      if (requestId === agentRequestRef.current) setError(formatRuntimeError(errorValue));
    }
  }, []);

  useEffect(() => {
    void refresh();
    let active = true;
    invoke<Settings>("get_settings")
      .then((nextSettings) => {
        if (!active) return;
        setSettings(nextSettings);
        applyTheme(nextSettings.theme);
      })
      .catch((errorValue) => {
        if (active) setError(formatRuntimeError(errorValue));
      });
    return () => {
      active = false;
    };
  }, [refresh]);

  // agents 变化后拉取各 Agent 的会话列表；空列表也要清理旧数据。
  useEffect(() => {
    if (agents.length === 0) {
      setSessionsByAgent({});
      return;
    }
    void refreshSessions(agents.map((agent) => agent.name));
  }, [agents, refreshSessions]);

  // 只订阅一次全局完成事件，读取 ref 避免因 Agent 列表更新反复注册监听器。
  useEffect(() => {
    let active = true;
    const unlisten = listen<AgentEventPayload>("agent-event", (event) => {
      if (!active) return;
      const normalized = normalizeAgentEvent(event.payload);
      if (normalized.event.type !== "agent_end") return;
      if (
        normalized.meta
        && (blockedSessionIdsRef.current.has(normalized.meta.sessionId)
          || normalized.meta.agentName !== selectedRef.current
          || (activeSessionRef.current?.sessionId
            && normalized.meta.sessionId !== activeSessionRef.current.sessionId))
      ) return;
      const infoRequestId = ++sessionInfoRequestRef.current;
      void invoke<SessionInfoView | null>("session_info")
        .then((info) => {
          if (active && infoRequestId === sessionInfoRequestRef.current) setActiveSession(info);
        })
        .catch((errorValue) => {
          if (active) setError(formatRuntimeError(errorValue));
        });
      void refreshSessions(agentsRef.current.map((agent) => agent.name));
    });
    return () => {
      active = false;
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [refreshSessions]);

  const openSession = async (agentName: string, sessionId: string) => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再切换会话");
      return;
    }
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    const previousSessionId = activeSessionRef.current?.sessionId;
    if (previousSessionId && previousSessionId !== sessionId) {
      blockedSessionIdsRef.current.add(previousSessionId);
    }
    blockedSessionIdsRef.current.delete(sessionId);
    await enqueueNavigation(requestId, async () => {
      let infoRequestId = 0;
      try {
        await invoke("open_session", { agentName, sessionId });
        if (requestId !== navigationRequestRef.current) {
          await invoke("new_session").catch(() => {});
          return;
        }
        setSelected(agentName);
        setCreating(false);
        setChatOpen(true);
        setSidebarOpen(false);
        setActiveSession(null);
        setChatKey((key) => key + 1);
        infoRequestId = ++sessionInfoRequestRef.current;
        const info = await invoke<SessionInfoView | null>("session_info");
        if (
          requestId === navigationRequestRef.current
          && infoRequestId === sessionInfoRequestRef.current
        ) {
          setActiveSession(info);
        } else {
          await invoke("new_session").catch(() => {});
        }
      } catch (errorValue) {
        if (
          requestId === navigationRequestRef.current
          && (infoRequestId === 0 || infoRequestId === sessionInfoRequestRef.current)
        ) {
          if (previousSessionId && previousSessionId !== sessionId) {
            blockedSessionIdsRef.current.delete(previousSessionId);
          }
          setError(formatRuntimeError(errorValue));
        } else {
          await invoke("new_session").catch(() => {});
        }
      }
    });
  };

  const startNewSession = async (agentName: string) => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再新建会话");
      return;
    }
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    const previousSessionId = activeSessionRef.current?.sessionId;
    if (previousSessionId) blockedSessionIdsRef.current.add(previousSessionId);
    await enqueueNavigation(requestId, async () => {
      try {
        await invoke("new_session");
        if (requestId !== navigationRequestRef.current) return;
        setSelected(agentName);
        setCreating(false);
        setChatOpen(true);
        setSidebarOpen(false);
        setActiveSession(null);
        setChatKey((key) => key + 1);
      } catch (errorValue) {
        if (requestId === navigationRequestRef.current) {
          if (previousSessionId) blockedSessionIdsRef.current.delete(previousSessionId);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
  };

  const createSessionFromChat = async (previousSessionId?: string): Promise<boolean> => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再新建会话");
      return false;
    }
    const requestId = ++navigationRequestRef.current;
    sessionInfoRequestRef.current += 1;
    if (previousSessionId) blockedSessionIdsRef.current.add(previousSessionId);
    let accepted = false;
    await enqueueNavigation(requestId, async () => {
      try {
        await invoke("new_session");
        accepted = requestId === navigationRequestRef.current;
      } catch (errorValue) {
        if (requestId === navigationRequestRef.current) {
          if (previousSessionId) blockedSessionIdsRef.current.delete(previousSessionId);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
    return accepted && requestId === navigationRequestRef.current;
  };

  const updateSettings = (next: Settings): Promise<void> => {
    const previous = settingsRef.current;
    const requestId = ++settingsSaveRequestRef.current;
    settingsRef.current = next;
    setSettings(next);
    applyTheme(next.theme);

    const save = settingsSaveQueueRef.current.then(async () => {
      try {
        await invoke("save_settings", { settings: next });
      } catch (errorValue) {
        if (requestId === settingsSaveRequestRef.current) {
          settingsRef.current = previous;
          setSettings(previous);
          if (previous) applyTheme(previous.theme);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
    settingsSaveQueueRef.current = save.catch(() => {});
    return save;
  };

  const current = agents.find((a) => a.name === selected) ?? null;

  const selectAgent = (agentName: string) => {
    if (chatRunning) {
      setError("Agent 正在运行，请先停止后再切换");
      return;
    }
    invalidateNavigation();
    setSelected(agentName);
    setCreating(false);
    setChatOpen(false);
    setSidebarOpen(false);
  };

  return (
    <div className={sidebarOpen ? "app sidebar-open" : "app"}>
      <aside className="sidebar">
        <div className="sidebar-header">
          <span className="logo">
            <span className="pi">π</span> pipi
          </span>
          <button
            type="button"
            className="icon-btn"
            title="设置"
            aria-label="打开设置"
            onClick={() => setSettingsOpen(true)}
          >
            ⚙
          </button>
        </div>
        <div className="sidebar-section">Agents</div>
        <nav className="agent-list">
          {agents.map((a) => {
            const isActiveAgent = selected === a.name && !creating;
            const sessions = sessionsByAgent[a.name] ?? [];
            return (
              <div key={a.name} className="agent-group">
                <div
                  className={`agent-item ${isActiveAgent && !chatOpen ? "active" : ""}`}
                  role="button"
                  tabIndex={0}
                  aria-current={isActiveAgent && !chatOpen ? "true" : undefined}
                  onClick={() => selectAgent(a.name)}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    selectAgent(a.name);
                  }}
                >
                  <div className="agent-row">
                    <div className="name">{a.name}</div>
                    <button
                      type="button"
                      className="icon-btn small"
                      title="新建会话"
                      aria-label={`为 ${a.name} 新建会话`}
                      onClick={(e) => {
                        e.stopPropagation();
                        startNewSession(a.name);
                      }}
                      onKeyDown={(e) => e.stopPropagation()}
                    >
                      ＋
                    </button>
                  </div>
                  {a.description && <div className="desc">{a.description}</div>}
                </div>
                <div className="session-list">
                  {sessions.map((sess) => (
                    <div
                      key={sess.id}
                      className={`session-item ${
                        activeSession?.agentName === a.name && activeSession?.sessionId === sess.id
                          ? "active"
                          : ""
                      }`}
                      role="button"
                      tabIndex={0}
                      aria-current={
                        activeSession?.agentName === a.name && activeSession?.sessionId === sess.id
                          ? "true"
                          : undefined
                      }
                      title={`${sess.title}（${sess.messageCount} 条消息）`}
                      onClick={() => openSession(a.name, sess.id)}
                      onKeyDown={(event) => {
                        if (event.key !== "Enter" && event.key !== " ") return;
                        event.preventDefault();
                        openSession(a.name, sess.id);
                      }}
                    >
                      <span className="session-title">{sess.title}</span>
                    </div>
                  ))}
                  {isActiveAgent && chatOpen && !activeSession && (
                    <div className="session-item new">（新会话）</div>
                  )}
                </div>
              </div>
            );
          })}
        </nav>
        <div className="sidebar-footer">
          <button
            type="button"
            className="primary wide"
            onClick={() => {
              if (chatRunning) {
                setError("Agent 正在运行，请先停止后再新建 Agent");
                return;
              }
              invalidateNavigation();
              setCreating(true);
            }}
          >
            ＋ 新建 Agent
          </button>
        </div>
      </aside>

      {sidebarOpen && (
        <button
          type="button"
          className="sidebar-backdrop"
          aria-label="关闭导航"
          onClick={() => setSidebarOpen(false)}
        />
      )}

      <main className="main">
        <div className="mobile-toolbar">
          <button
            type="button"
            className="icon-btn"
            aria-label="打开导航"
            title="打开导航"
            onClick={() => setSidebarOpen(true)}
          >
            ☰
          </button>
          <span>{current?.name ?? (creating ? "新建 Agent" : "Pipi")}</span>
        </div>
        {error && (
          <div className="error" role="alert">
            <span>{error}</span>
            <button type="button" className="error-dismiss" onClick={() => setError(null)} aria-label="关闭错误提示">
              ✕
            </button>
          </div>
        )}
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
              key={`${current.name}-${chatKey}`}
              agent={current}
              blockedSessionIds={[...blockedSessionIdsRef.current]}
              onBack={() => {
                if (chatRunning) {
                  setError("Agent 正在运行，请先停止后再返回");
                  return;
                }
                invalidateNavigation();
                setChatOpen(false);
                setSidebarOpen(false);
              }}
              onError={setError}
              onNewSession={createSessionFromChat}
              onRunningChange={setChatRunning}
              onSessionReset={() => {
                setActiveSession(null);
                void refreshSessions([current.name]);
              }}
            />
          ) : (
            <AgentDetail
              key={current.name}
              agent={current}
              providers={settings.providers}
              onSaved={refresh}
              onChat={() => void startNewSession(current.name)}
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
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
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
  onError: (msg: string | null) => void;
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
    onError(null);
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
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
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
  const [draft, setDraft] = useState(settings);
  const draftRef = useRef(settings);

  useEffect(() => {
    draftRef.current = settings;
    setDraft(settings);
  }, [settings]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const commit = (update: (previous: Settings) => Settings) => {
    const next = update(draftRef.current);
    draftRef.current = next;
    setDraft(next);
    void onChange(next);
  };

  const saveProvider = (provider: ProviderConfig) => {
    commit((previous) => {
      const providers = [...previous.providers];
      const index = providers.findIndex((item) => item.id === provider.id);
      if (index >= 0) providers[index] = provider;
      else providers.push(provider);
      return { ...previous, providers };
    });
    setEditing(null);
  };

  const deleteProvider = (id: string) => {
    if (!confirm(`删除提供商「${id}」？（已绑定它的 Agent 不受影响，但需重新配置）`)) return;
    commit((previous) => ({
      ...previous,
      providers: previous.providers.filter((provider) => provider.id !== id),
      defaultProviderId: previous.defaultProviderId === id ? null : previous.defaultProviderId,
    }));
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="settings-title">设置</h2>
          <button type="button" className="icon-btn" onClick={onClose} title="关闭">
            ✕
          </button>
        </div>

        <div className="modal-section">
          <span className="label">主题</span>
          <div className="theme-row">
            <ThemeOption
              active={draft.theme === "dark"}
              name="深色"
              swatch={["#111111", "#181818", "#0169CC", "#FCFCFC"]}
              onClick={() => commit((previous) => ({ ...previous, theme: "dark" }))}
            />
            <ThemeOption
              active={draft.theme === "light"}
              name="浅色"
              swatch={["#FCFCFC", "#FFFFFF", "#0169CC", "#111111"]}
              onClick={() => commit((previous) => ({ ...previous, theme: "light" }))}
            />
          </div>
        </div>

        <div className="modal-section">
          <span className="label">模型提供商</span>
          {draft.providers.map((provider) => {
            const status = keyStatus(provider);
            return (
              <div className="provider-row" key={provider.id}>
                <div className="info">
                  <div className="p-name">
                    {provider.name}
                    <span className="badge">{API_LABELS[provider.api]}</span>
                    <span className={`badge ${status.warn ? "warn" : "neutral"}`}>
                      {status.label}
                    </span>
                    {draft.defaultProviderId === provider.id && (
                      <span className="badge">默认</span>
                    )}
                  </div>
                  <div className="p-url mono">{provider.baseUrl}</div>
                </div>
                <div className="p-actions">
                  <button type="button" className="link" onClick={() => setEditing(provider)}>
                    编辑
                  </button>
                  {draft.defaultProviderId !== provider.id && (
                    <button
                      type="button"
                      className="link"
                      onClick={() => commit((previous) => ({ ...previous, defaultProviderId: provider.id }))}
                    >
                      设为默认
                    </button>
                  )}
                  <button type="button" className="link danger" onClick={() => deleteProvider(provider.id)}>
                    删除
                  </button>
                </div>
              </div>
            );
          })}

          {editing === null && (
            <button type="button" className="ghost" onClick={() => setEditing("new")}>
              ＋ 添加提供商
            </button>
          )}

          {editing !== null && (
            <ProviderForm
              initial={editing === "new" ? null : editing}
              existingIds={draft.providers.map((provider) => provider.id)}
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
    <button
      type="button"
      className={`theme-option ${active ? "active" : ""}`}
      aria-pressed={active}
      onClick={onClick}
    >
      <div className="swatch">
        {swatch.map((color) => (
          <span key={color} style={{ background: color }} />
        ))}
      </div>
      <div className="name">
        {name}
        {active && <span style={{ color: "var(--accent-text)" }}> ✓</span>}
      </div>
    </button>
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
  return slug || "provider-new";
}
