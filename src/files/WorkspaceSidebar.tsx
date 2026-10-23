import { useContext, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import { IconFolder } from "../icons";
import { getWorkspaceBasename, type WorkspaceInfo } from "../app/workspaces";
import { invoke } from "../platform";
import { formatSize } from "./FilePreview";
import { useFileResource, type WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceListing } from "./types";

export function WorkspaceSidebar({
  state,
  workspaces = [],
  activeWorkspace = "all",
  onSelectWorkspace,
}: {
  state: WorkspaceFilesState;
  workspaces?: WorkspaceInfo[];
  activeWorkspace?: string;
  onSelectWorkspace?: (path: string) => void;
}) {
  const { closeNavigation } = useContext(LayoutContext);
  const [path, setPath] = useState("");
  const { target, mode, store, view, changes } = state;
  const open = (path: string) => { store?.open(path); closeNavigation(); };
  const changeFiles = changes.value?.files.filter(item => item.path.toLowerCase().includes(view.query.toLowerCase())) ?? [];
  const groups = [
    { name: "已暂存", files: changeFiles.filter(file => file.status !== "??" && file.status[0] !== " ") },
    { name: "未暂存", files: changeFiles.filter(file => file.status !== "??" && file.status[1] !== " ") },
    { name: "未跟踪", files: changeFiles.filter(file => file.status === "??") },
  ];

  const currentWorkspacePath = target?.workspace || "";
  const currentWorkspaceName = target?.name || getWorkspaceBasename(currentWorkspacePath);

  return <div className="file-workbench workspace-files-sidebar">
    <div className="file-workbench-header">
      <strong>{mode === "files" ? "File · 文件" : "Git · 改动"}</strong>
      <button type="button" className="btn small ghost" onClick={state.refresh} disabled={!target}>刷新</button>
    </div>

    {workspaces.length > 1 && activeWorkspace === "all" ? (
      <label className="workspace-agent-picker">工作区
        <select
          className="text-input"
          aria-label="选择工作区"
          value={currentWorkspacePath}
          onChange={event => onSelectWorkspace?.(event.target.value)}
        >
          {workspaces.map(w => {
            const name = w.name || getWorkspaceBasename(w.path);
            return (
              <option value={w.path} key={w.path}>
                {name}
              </option>
            );
          })}
        </select>
      </label>
    ) : currentWorkspacePath ? (
      <div className="workspace-current-badge" title={currentWorkspacePath}>
        <IconFolder />
        <span className="workspace-current-name">{currentWorkspaceName}</span>
      </div>
    ) : null}

    {!target ? <p className="file-notice">暂无可预览的工作区，请先在工作区列表创建或关联工作区。</p> : <>
      <input className="text-input file-filter" aria-label="筛选文件名" value={view.query} placeholder={mode === "files" ? "筛选已展开的文件…" : "筛选改动文件…"} onChange={event => store?.setQuery(event.target.value)} />
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
        <input className="text-input" aria-label="打开工作区文件路径" placeholder="输入文件路径…" value={path} onChange={event => setPath(event.target.value)} /><button type="submit" className="btn" disabled={!path}>打开</button>
      </form> : <p className="file-notice">预览以 HEAD 对比当前工作树，合并展示已暂存和未暂存差异。</p>}
    </>}
  </div>;
}

function Directory({ path, depth, state, onOpen }: { path: string; depth: number; state: WorkspaceFilesState; onOpen: (path: string) => void }) {
  const { target, store, view } = state;
  const lookupTarget = target?.workspace || target?.name || "";
  const nested = useFileResource<WorkspaceListing>(path && state.active && store ? JSON.stringify([store.key, path]) : null, state.revision,
    () => invoke("list_workspace_files", { agentName: lookupTarget, path }));
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
