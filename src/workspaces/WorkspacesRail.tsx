// 工作区右栏：当前范围摘要与所有工作区的快速切换。

import { useRightRail, RightRail } from "../app/right-rail";
import {
  getWorkspaceBasename,
  normalizeWorkspacePath,
  type WorkspaceInfo,
} from "../app/workspaces";
import { IconAgent, IconFolder, IconGit, IconHome, IconPlus } from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";

export function WorkspacesRail({
  controller,
  workspaces,
  activeWorkspace,
  agents,
  sessionsByAgent,
  onSelectWorkspace,
  onOpenCreateModal,
  onCreateAgent,
  onNavigateSection,
}: {
  controller: ReturnType<typeof useRightRail>;
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  agents: AgentDefinition[];
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  onSelectWorkspace: (workspace: string) => void;
  onOpenCreateModal: () => void;
  onCreateAgent: (defaultWorkspace?: string) => void;
  onNavigateSection: (section: "agents" | "files" | "git") => void;
}) {
  const isAll = activeWorkspace === "all";
  const current = workspaces.find((workspace) => workspace.path === activeWorkspace);
  const displayName = isAll ? "全部工作区" : current?.name || getWorkspaceBasename(activeWorkspace);
  const currentNorm = normalizeWorkspacePath(activeWorkspace);
  const primaryAgents = agents.filter((agent) => !agent.subagent);
  const scopedAgents = isAll ? primaryAgents : primaryAgents.filter((agent) => normalizeWorkspacePath(agent.workspace) === currentNorm);
  const sessions = scopedAgents.reduce((total, agent) => total + (sessionsByAgent[agent.name]?.length ?? 0), 0);

  return <RightRail
    controller={controller}
    id="workspaces-rail"
    label={displayName}
    subtitle={`${isAll ? workspaces.length : scopedAgents.length} 个工作区范围 · ${scopedAgents.length} 个 Agent`}
    icon={isAll ? <IconHome /> : <IconFolder />}
    actions={<button type="button" className="icon-btn" title="新建工作区" aria-label="新建工作区" onClick={onOpenCreateModal}>
      <IconPlus />
    </button>}
    tabs={[
      {
        id: "scope",
        label: "范围",
        content: <Scope workspaceName={displayName} path={isAll ? "全部工作区" : activeWorkspace} isAll={isAll}
          workspaces={workspaces.length} agents={scopedAgents.length} sessions={sessions}
          onOpenCreateModal={onOpenCreateModal} onCreateAgent={onCreateAgent}
          onNavigateSection={onNavigateSection} />,
      },
      {
        id: "switch",
        label: "切换",
        badge: workspaces.length,
        content: <WorkspaceList workspaces={workspaces} activeWorkspace={activeWorkspace}
          onSelectWorkspace={onSelectWorkspace} />,
      },
    ]}
  />;
}

function Scope({ workspaceName, path, isAll, workspaces, agents, sessions, onOpenCreateModal, onCreateAgent, onNavigateSection }: {
  workspaceName: string;
  path: string;
  isAll: boolean;
  workspaces: number;
  agents: number;
  sessions: number;
  onOpenCreateModal: () => void;
  onCreateAgent: (defaultWorkspace?: string) => void;
  onNavigateSection: (section: "agents" | "files" | "git") => void;
}) {
  return <>
    <section className="right-rail-section">
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        {isAll ? <IconHome /> : <IconFolder />}
        <span className="right-rail-item-main">{workspaceName}</span>
      </div>
      <div className="right-rail-kv"><span>路径</span><span className="mono">{path}</span></div>
      <div className="right-rail-kv"><span>范围</span><span>{isAll ? "全部工作区" : "单个工作区"}</span></div>
    </section>
    <section className="right-rail-section">
      <h3>概况</h3>
      <div className="right-rail-stats">
        <div className="right-rail-stat"><span>工作区</span><strong>{workspaces}</strong></div>
        <div className="right-rail-stat"><span>Agent</span><strong>{agents}</strong></div>
        <div className="right-rail-stat"><span>会话</span><strong>{sessions}</strong></div>
        <div className="right-rail-stat"><span>模式</span><strong>{isAll ? "全景" : "工作区"}</strong></div>
      </div>
    </section>
    <section className="right-rail-section">
      <h3>快捷操作</h3>
      <div className="right-rail-actions-row">
        <button type="button" className="btn small" onClick={onOpenCreateModal}><IconPlus /> 新建工作区</button>
        <button type="button" className="btn small ghost" onClick={() => onCreateAgent(isAll ? undefined : path)}>
          <IconAgent /> 新建 Agent
        </button>
        {!isAll && <>
          <button type="button" className="btn small ghost" onClick={() => onNavigateSection("files")}><IconFolder /> File</button>
          <button type="button" className="btn small ghost" onClick={() => onNavigateSection("git")}><IconGit /> Git</button>
        </>}
        {isAll && <button type="button" className="btn small ghost" onClick={() => onNavigateSection("agents")}><IconAgent /> 全部 Agent</button>}
      </div>
      <p className="right-rail-note">工作区只聚合主 Agent；Subagent 跟随它所属的 Agent，不单独计入工作区。</p>
    </section>
  </>;
}

function WorkspaceList({ workspaces, activeWorkspace, onSelectWorkspace }: {
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  onSelectWorkspace: (workspace: string) => void;
}) {
  return <section className="right-rail-section">
    <h3>所有工作区 · {workspaces.length}</h3>
    <div className="right-rail-list">
      <button type="button" className={`right-rail-item${activeWorkspace === "all" ? " active" : ""}`} onClick={() => onSelectWorkspace("all")}>
        <IconHome />
        <span className="right-rail-item-main">全部工作区</span>
        <small>{workspaces.reduce((total, item) => total + item.agentCount, 0)} Agent</small>
      </button>
      {workspaces.map((workspace) => <button type="button" key={workspace.path}
        className={`right-rail-item${workspace.path === activeWorkspace ? " active" : ""}`}
        title={workspace.path} onClick={() => onSelectWorkspace(workspace.path)}>
        <IconFolder />
        <span className="right-rail-item-main">{workspace.name || getWorkspaceBasename(workspace.path)}</span>
        <small>{workspace.agentCount} Agent</small>
      </button>)}
    </div>
  </section>;
}
