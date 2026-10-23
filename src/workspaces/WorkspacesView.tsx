import { NavigationToggle } from "../app/AppNavigation";
import AgentAvatar from "../components/AgentAvatar";
import {
  IconAgent,
  IconFolder,
  IconGit,
  IconHome,
  IconPlus,
} from "../icons";
import {
  getWorkspaceBasename,
  normalizeWorkspacePath,
  type WorkspaceInfo,
} from "../app/workspaces";
import type { AgentDefinition, SessionSummaryView } from "../types";

interface WorkspacesViewProps {
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  agents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  onSelectWorkspace: (workspace: string) => void;
  onOpenCreateModal: () => void;
  onSelectAgent: (agentName: string) => void;
  onConfigureAgent: (agentName: string) => void;
  onOpenChat: (agentName: string, sessionId: string | null) => void;
  onCreateAgent: (defaultWorkspace?: string) => void;
  onNavigateSection: (section: "agents" | "subagents" | "files" | "git") => void;
}

export function WorkspacesView({
  workspaces,
  activeWorkspace,
  agents,
  sessionsByAgent,
  onSelectWorkspace,
  onOpenCreateModal,
  onSelectAgent,
  onConfigureAgent,
  onOpenChat,
  onCreateAgent,
  onNavigateSection,
}: WorkspacesViewProps) {
  const isAll = activeWorkspace === "all";
  const currentWorkspace = workspaces.find((w) => w.path === activeWorkspace);
  const displayName = isAll
    ? "全部工作区"
    : currentWorkspace?.name || getWorkspaceBasename(activeWorkspace);

  // 属于当前工作区的 Agents（工作区只关注主 Agent，不看 Subagent）
  const currentNorm = normalizeWorkspacePath(activeWorkspace);
  const primaryAgents = agents.filter((a) => !a.subagent);
  const workspaceAgents = isAll
    ? primaryAgents
    : primaryAgents.filter(
        (a) => normalizeWorkspacePath(a.workspace) === currentNorm,
      );

  const totalSessionsCount = workspaceAgents.reduce(
    (acc, a) => acc + (sessionsByAgent[a.name]?.length ?? 0),
    0,
  );

  return (
    <div className="workspaces-view">
      <header className="workspace-toolbar">
        <NavigationToggle />
        <span className="toolbar-divider" />
        <span className="workspaces-toolbar-title">
          {isAll ? "工作区 · 全部" : `工作区 · ${displayName}`}
        </span>
        <span className="spacer" />
        <button
          type="button"
          className="btn small"
          onClick={onOpenCreateModal}
        >
          <IconPlus />
          <span>新建工作区</span>
        </button>
      </header>

      <div className="workspaces-view-scroll">
        {isAll ? (
          // ================= 全部工作区视图 =================
          <div className="workspaces-all-container">
            <div className="workspaces-hero-card">
              <div className="workspaces-hero-icon-all">
                <IconHome />
              </div>
              <div className="workspaces-hero-content">
                <h2>全部工作区</h2>
                <p className="workspaces-hero-desc">
                  当前处于全景模式，聚合展示全部工作区的 Agent 与会话内容。共发现{" "}
                  <strong>{workspaces.length}</strong> 个工作区，关联{" "}
                  <strong>{primaryAgents.length}</strong> 个 Agent。
                </p>
                <div className="workspaces-hero-actions">
                  <button
                    type="button"
                    className="btn primary"
                    onClick={onOpenCreateModal}
                  >
                    <IconPlus />
                    <span>新建工作区</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("agents")}
                  >
                    <IconAgent />
                    <span>查看所有 Agent</span>
                  </button>
                </div>
              </div>
            </div>

            <div className="workspaces-section">
              <div className="workspaces-section-header">
                <h3>所有工作区清单 ({workspaces.length})</h3>
              </div>

              <div className="workspaces-grid">
                {workspaces.map((w) => {
                  const wAgents = primaryAgents.filter(
                    (a) =>
                      normalizeWorkspacePath(a.workspace) ===
                      normalizeWorkspacePath(w.path),
                  );
                  const name = w.name || getWorkspaceBasename(w.path);
                  return (
                    <div key={w.path} className="workspace-card">
                      <div className="workspace-card-head">
                        <div className="workspace-card-icon">
                          <IconFolder />
                        </div>
                        <div className="workspace-card-titles">
                          <h4 className="workspace-card-name" title={name}>
                            {name}
                          </h4>
                          <span className="workspace-card-path" title={w.path}>
                            {w.path}
                          </span>
                        </div>
                        <span className="workspace-card-badge">
                          {wAgents.length} Agent
                        </span>
                      </div>

                      <div className="workspace-card-body">
                        {wAgents.length > 0 ? (
                          <div className="workspace-card-agents">
                            {wAgents.slice(0, 4).map((a) => (
                              <span
                                key={a.name}
                                className="workspace-card-agent-tag"
                                onClick={() => onSelectAgent(a.name)}
                                title={`点击查看 ${a.name}`}
                              >
                                {a.name}
                              </span>
                            ))}
                            {wAgents.length > 4 && (
                              <span className="workspace-card-agent-more">
                                +{wAgents.length - 4}
                              </span>
                            )}
                          </div>
                        ) : (
                          <span className="workspace-card-empty-agents">
                            暂无关联 Agent
                          </span>
                        )}
                      </div>

                      <div className="workspace-card-actions">
                        <button
                          type="button"
                          className="btn small primary"
                          onClick={() => onSelectWorkspace(w.path)}
                        >
                          <span>切换到此工作区</span>
                        </button>
                        <button
                          type="button"
                          className="btn small ghost"
                          title="为此工作区新建 Agent"
                          onClick={() => onCreateAgent(w.path)}
                        >
                          <IconPlus />
                          <span>新建 Agent</span>
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        ) : (
          // ================= 单个工作区视图 =================
          <div className="workspace-detail-container">
            <div className="workspaces-hero-card">
              <div className="workspaces-hero-icon">
                <IconFolder />
              </div>
              <div className="workspaces-hero-content">
                <div className="workspaces-hero-title-row">
                  <h2>{displayName}</h2>
                  {currentWorkspace?.custom && (
                    <span className="workspace-tag-custom">自定义工作区</span>
                  )}
                </div>
                <div className="workspaces-hero-path" title={activeWorkspace}>
                  {activeWorkspace}
                </div>
                <div className="workspaces-hero-stats">
                  <span className="stat-pill">
                    <strong>{workspaceAgents.length}</strong> 个关联 Agent
                  </span>
                  <span className="stat-pill">
                    <strong>{totalSessionsCount}</strong> 个活跃会话
                  </span>
                </div>
                <div className="workspaces-hero-actions">
                  <button
                    type="button"
                    className="btn primary"
                    onClick={() => onCreateAgent(activeWorkspace)}
                  >
                    <IconPlus />
                    <span>为此工作区新建 Agent</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("agents")}
                  >
                    <IconAgent />
                    <span>查看关联 Agent</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("files")}
                  >
                    <IconFolder />
                    <span>浏览文件</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onNavigateSection("git")}
                  >
                    <IconGit />
                    <span>查看 Git</span>
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => onSelectWorkspace("all")}
                  >
                    <IconHome />
                    <span>切换回全部工作区</span>
                  </button>
                </div>
              </div>
            </div>

            {/* 工作区关联 Agent */}
            <div className="workspaces-section">
              <div className="workspaces-section-header">
                <h3>工作区内的 Agent ({workspaceAgents.length})</h3>
                <span className="spacer" />
                <button
                  type="button"
                  className="btn small ghost"
                  onClick={() => onCreateAgent(activeWorkspace)}
                >
                  <IconPlus />
                  <span>添加 Agent</span>
                </button>
              </div>

              {workspaceAgents.length === 0 ? (
                <div className="workspace-empty-card">
                  <p>当前工作区下还没有关联任何 Agent。</p>
                  <button
                    type="button"
                    className="btn primary"
                    onClick={() => onCreateAgent(activeWorkspace)}
                  >
                    <IconPlus />
                    <span>创建第一个关联 Agent</span>
                  </button>
                </div>
              ) : (
                <div className="workspaces-agents-grid">
                  {workspaceAgents.map((agent) => (
                    <div key={agent.name} className="workspace-agent-card">
                      <div className="workspace-agent-header">
                        <AgentAvatar name={agent.name} subagent={agent.subagent} />
                        <div className="workspace-agent-info">
                          <h4 className="workspace-agent-name">{agent.name}</h4>
                          <span className="workspace-agent-model">
                            {agent.model ?? "默认模型"}
                          </span>
                        </div>
                      </div>
                      <p className="workspace-agent-desc">
                        {agent.description || "暂无描述"}
                      </p>
                      <div className="workspace-agent-actions">
                        <button
                          type="button"
                          className="btn small primary"
                          onClick={() => onOpenChat(agent.name, null)}
                        >
                          发起会话
                        </button>
                        <button
                          type="button"
                          className="btn small ghost"
                          onClick={() => onSelectAgent(agent.name)}
                        >
                          概览
                        </button>
                        <button
                          type="button"
                          className="btn small ghost"
                          onClick={() => onConfigureAgent(agent.name)}
                        >
                          配置
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>

            {/* 快速切换到其他工作区 */}
            {workspaces.length > 1 && (
              <div className="workspaces-section other-workspaces">
                <div className="workspaces-section-header">
                  <h3>快速切换到其他工作区</h3>
                </div>
                <div className="other-workspaces-pills">
                  {workspaces
                    .filter((w) => w.path !== activeWorkspace)
                    .map((w) => {
                      const name = w.name || getWorkspaceBasename(w.path);
                      return (
                        <button
                          key={w.path}
                          type="button"
                          className="other-workspace-pill"
                          onClick={() => onSelectWorkspace(w.path)}
                          title={w.path}
                        >
                          <IconFolder />
                          <span className="pill-name">{name}</span>
                          <span className="pill-badge">{w.agentCount}</span>
                        </button>
                      );
                    })}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
