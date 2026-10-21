import { useContext, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import { invoke } from "../platform";
import type { AgentDefinition } from "../types";
import { formatSize } from "./FilePreview";
import { useFileResource, type WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceListing } from "./types";

export function WorkspaceSidebar({ state, agents, onSelectAgent }: {
  state: WorkspaceFilesState;
  agents: AgentDefinition[];
  onSelectAgent: (name: string | null) => void;
}) {
  const { closeNavigation } = useContext(LayoutContext);
  const [path, setPath] = useState("");
  const { agent, mode, store, view, changes } = state;
  const open = (path: string) => { store?.open(path); closeNavigation(); };
  const changeFiles = changes.value?.files.filter(item => item.path.toLowerCase().includes(view.query.toLowerCase())) ?? [];
  const groups = [
    { name: "已暂存", files: changeFiles.filter(file => file.status !== "??" && file.status[0] !== " ") },
    { name: "未暂存", files: changeFiles.filter(file => file.status !== "??" && file.status[1] !== " ") },
    { name: "未跟踪", files: changeFiles.filter(file => file.status === "??") },
  ];
  return <div className="file-workbench workspace-files-sidebar">
    <div className="file-workbench-header"><strong>{mode === "files" ? "File · 文件" : "Git · 改动"}</strong><button type="button" onClick={state.refresh} disabled={!agent}>刷新</button></div>
    <label className="workspace-agent-picker">Agent 工作区
      <select aria-label="选择 Agent 工作区" value={agent?.name ?? ""} onChange={event => onSelectAgent(event.target.value || null)}>
        <option value="">请选择 Agent</option>
        {agents.map(item => <option value={item.name} key={item.name}>{item.name}{item.subagent ? " · Subagent" : ""}</option>)}
      </select>
    </label>
    {!agent ? <p className="file-notice">选择 Agent 后浏览它的工作目录，无需先打开会话。</p> : <>
      <input className="file-filter" aria-label="筛选文件名" value={view.query} placeholder={mode === "files" ? "筛选已展开的文件…" : "筛选改动文件…"} onChange={event => store?.setQuery(event.target.value)} />
      {mode === "files" && <label className="file-hidden"><input type="checkbox" checked={view.showHidden} onChange={event => store?.setHidden(event.target.checked)} />显示隐藏文件</label>}
      <div className="workspace-file-list" aria-label={mode === "files" ? "工作区文件树" : "Git 改动列表"}>
        {mode === "files" ? <Directory key={store?.key} path="" depth={0} state={state} onOpen={open} />
          : changes.loading ? <p className="file-notice" role="status">读取 Git 状态…</p>
          : changes.error ? <p className="file-error" role="alert">{changes.error}</p>
          : !changes.value?.isRepo ? <p className="file-notice">此工作目录不属于 Git 仓库。</p>
          : !changes.value.files.length ? <p className="file-notice">工作区没有未提交改动。</p>
          : !changeFiles.length ? <p className="file-notice">没有匹配的改动文件。</p>
          : groups.filter(group => group.files.length).map(group => <section key={group.name} className="workspace-git-group"><h3>{group.name}<small>{group.files.length}</small></h3>
            {group.files.map(file => <button type="button" className={`file-row${file.path === view.selected ? " active" : ""}`} key={file.path} title={file.path} aria-pressed={file.path === view.selected} onClick={() => open(file.path)}><code>{file.status}</code><span>{file.path}</span></button>)}
          </section>)}
      </div>
      {mode === "files" ? <form className="file-open-path" onSubmit={event => { event.preventDefault(); if (path) open(path); }}>
        <input aria-label="打开工作区文件路径" placeholder="输入文件路径…" value={path} onChange={event => setPath(event.target.value)} /><button type="submit" disabled={!path}>打开</button>
      </form> : <p className="file-notice">预览以 HEAD 对比当前工作树，合并展示已暂存和未暂存差异。</p>}
    </>}
  </div>;
}

function Directory({ path, depth, state, onOpen }: { path: string; depth: number; state: WorkspaceFilesState; onOpen: (path: string) => void }) {
  const { agent, store, view } = state;
  const nested = useFileResource<WorkspaceListing>(path && state.active && store ? JSON.stringify([store.key, path]) : null, state.revision,
    () => invoke("list_workspace_files", { agentName: agent!.name, path }));
  const resource = path ? nested : state.rootListing;
  if (resource.loading) return <p className="file-notice" role="status">读取目录…</p>;
  if (resource.error) return <p className="file-error" role="alert">{resource.error}</p>;
  const entries = resource.value?.entries.filter(item => (view.showHidden || !item.name.startsWith("."))
    && (item.kind === "directory" || item.name.toLowerCase().includes(view.query.toLowerCase()))) ?? [];
  return <>
    {!entries.length && <p className="file-notice">{view.query ? "没有匹配的文件。" : "空目录"}</p>}
    {entries.map(item => {
      const directory = item.kind === "directory";
      const expanded = directory && view.expanded.includes(item.path);
      return <div key={item.path}>
        <button type="button" className={`file-row${!directory && item.path === view.selected ? " active" : ""}`} style={{ paddingLeft: 6 + depth * 14 }}
          title={item.kind === "symlink" ? `${item.path}（符号链接不允许预览）` : item.path} aria-expanded={directory ? expanded : undefined}
          aria-pressed={directory ? undefined : item.path === view.selected} disabled={!["directory", "file"].includes(item.kind)}
          onClick={() => directory ? store?.toggleFolder(item.path) : onOpen(item.path)}>
          <span aria-hidden="true">{directory ? expanded ? "▾" : "▸" : item.kind === "symlink" ? "↗" : "·"}</span><span>{item.name}</span>{!directory && <small>{formatSize(item.size)}</small>}
        </button>
        {expanded && <Directory path={item.path} depth={depth + 1} state={state} onOpen={onOpen} />}
      </div>;
    })}
    {resource.value?.truncated && <p className="file-notice">此目录超过 2000 项，仅显示部分内容。</p>}
  </>;
}
