import { useContext, useMemo, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import { useCopyFeedback } from "../components/use-copy";
import { getWorkspaceBasename, type WorkspaceInfo } from "../app/workspaces";
import {
  IconCaret,
  IconCheck,
  IconCollapseAll,
  IconCopy,
  IconFile,
  IconFolder,
  IconGit,
  IconGitBranch,
  IconList,
  IconRefresh,
  IconSearch,
  IconTree,
} from "../icons";
import { invoke } from "../platform";
import { gitStatusMeta } from "./diff";
import { FileGlyph } from "./FileGlyph";
import { formatSize } from "./FilePreview";
import { useFileResource, type WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceChange, WorkspaceListing } from "./types";

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
  const [gitViewMode, setGitViewMode] = useState<"list" | "tree">("list");
  const [selectedRepo, setSelectedRepo] = useState<string>("all");
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(new Set());
  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  const clipboard = useCopyFeedback();

  const { target, mode, store, view, changes } = state;
  const open = (filePath: string) => {
    store?.open(filePath);
    closeNavigation();
  };

  const handleCopy = (e: React.MouseEvent, filePath: string) => {
    e.stopPropagation();
    setCopiedPath(filePath);
    void clipboard.copy(filePath);
  };

  const repositories = changes.value?.repositories ?? [];
  const multiRepo = repositories.length > 1;

  const repoFilteredFiles = useMemo(() => {
    const all = changes.value?.files ?? [];
    if (selectedRepo === "all" || !multiRepo) return all;
    return all.filter((file) => file.repository === selectedRepo);
  }, [changes.value?.files, selectedRepo, multiRepo]);

  const changeFiles = useMemo(() => {
    return repoFilteredFiles.filter((item) =>
      item.path.toLowerCase().includes(view.query.toLowerCase())
    );
  }, [repoFilteredFiles, view.query]);

  const summary = summarize(repoFilteredFiles);

  const groups = [
    {
      id: "staged",
      name: "已暂存的更改",
      tone: "staged" as const,
      files: changeFiles.filter((file) => gitStatusMeta(file.status).staged),
    },
    {
      id: "unstaged",
      name: "更改",
      tone: "unstaged" as const,
      files: changeFiles.filter((file) => {
        const meta = gitStatusMeta(file.status);
        return meta.tone !== "untracked" && meta.unstaged && !meta.staged;
      }),
    },
    {
      id: "untracked",
      name: "未跟踪的文件",
      tone: "untracked" as const,
      files: changeFiles.filter(
        (file) => gitStatusMeta(file.status).tone === "untracked"
      ),
    },
  ];

  const currentWorkspacePath = target?.workspace || "";
  const currentWorkspaceName = target?.name || getWorkspaceBasename(currentWorkspacePath);
  const title = mode === "files" ? "文件" : "源代码管理";
  const detail =
    mode === "files"
      ? `${state.rootListing.value?.entries.length ?? 0} 个顶层条目`
      : changes.value?.isRepo === false
        ? "非 Git 仓库"
        : `${summary.total} 个更改`;

  const toggleGroup = (id: string) => {
    setCollapsedGroups((prev) => ({ ...prev, [id]: !prev[id] }));
  };

  const toggleFolder = (folderPath: string) => {
    setCollapsedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(folderPath)) next.delete(folderPath);
      else next.add(folderPath);
      return next;
    });
  };

  const handleCollapseAll = () => {
    if (gitViewMode === "tree") {
      // 在树状模式下，折叠所有文件夹
      const allFolderPaths = new Set<string>();
      for (const group of groups) {
        collectFolderPaths(buildGitTree(group.files), allFolderPaths);
      }
      setCollapsedFolders(allFolderPaths);
    } else {
      // 在列表模式下，折叠所有更改分组
      setCollapsedGroups({
        staged: true,
        unstaged: true,
        untracked: true,
      });
    }
  };

  const activeRepo =
    selectedRepo !== "all"
      ? repositories.find((r) => r.name === selectedRepo)
      : repositories.length === 1
        ? repositories[0]
        : undefined;

  return (
    <div className={`file-workbench workspace-files-sidebar${mode === "git" ? " vscode-scm-sidebar" : ""}`}>
      {/* 顶部标题栏：VS Code 风格小字全大写与右侧工具栏 */}
      <div className="file-workbench-header scm-header-bar">
        <div className="workspace-sidebar-title scm-title">
          <span className={`workspace-sidebar-icon ${mode}`}>
            {mode === "files" ? <IconFile /> : <IconGit />}
          </span>
          <div className="scm-title-text">
            <strong>{title}</strong>
            <small>{detail}</small>
          </div>
        </div>
        <div className="workspace-sidebar-actions scm-header-actions">
          {mode === "git" && (
            <>
              <button
                type="button"
                className={`icon-btn small${gitViewMode === "tree" ? " active" : ""}`}
                title="切换到树状视图 (View as Tree)"
                aria-label="树状视图"
                aria-pressed={gitViewMode === "tree"}
                onClick={() => setGitViewMode("tree")}
              >
                <IconTree />
              </button>
              <button
                type="button"
                className={`icon-btn small${gitViewMode === "list" ? " active" : ""}`}
                title="切换到列表视图 (View as List)"
                aria-label="列表视图"
                aria-pressed={gitViewMode === "list"}
                onClick={() => setGitViewMode("list")}
              >
                <IconList />
              </button>
              <button
                type="button"
                className="icon-btn small"
                title="全部折叠 (Collapse All)"
                aria-label="全部折叠"
                onClick={handleCollapseAll}
              >
                <IconCollapseAll />
              </button>
            </>
          )}
          <button
            type="button"
            className="icon-btn small"
            title="刷新 (Refresh)"
            aria-label="刷新"
            onClick={state.refresh}
            disabled={!target}
          >
            <IconRefresh />
          </button>
        </div>
      </div>

      {workspaces.length > 1 && activeWorkspace === "all" ? (
        <label className="workspace-agent-picker">
          工作区
          <select
            className="text-input"
            aria-label="选择工作区"
            value={currentWorkspacePath}
            onChange={(event) => onSelectWorkspace?.(event.target.value)}
          >
            {workspaces.map((w) => {
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

      {!target ? (
        <div className="workspace-sidebar-empty">
          <IconFolder />
          <p>暂无可预览的工作区</p>
          <small>请先在工作区列表创建或关联工作区。</small>
        </div>
      ) : (
        <>
          {mode === "git" && (
            <div className="scm-repo-bar" hidden={!multiRepo && !activeRepo}>
              {multiRepo ? (
                <label className="scm-repo-picker">
                  <span className="scm-repo-picker-label">
                    <IconGitBranch /> 仓库
                  </span>
                  <select
                    className="text-input scm-repo-select"
                    aria-label="选择 Git 仓库"
                    value={selectedRepo}
                    onChange={(e) => setSelectedRepo(e.target.value)}
                  >
                    <option value="all">全部仓库 ({repositories.length})</option>
                    {repositories.map((r) => (
                      <option value={r.name} key={r.path || r.name}>
                        {r.name}
                        {r.branch ? ` (${r.branch})` : ""}
                      </option>
                    ))}
                  </select>
                </label>
              ) : activeRepo ? (
                <div className="scm-branch-badge" title={`仓库路径：${activeRepo.path || "."}`}>
                  <IconGitBranch />
                  <span className="scm-repo-name">{activeRepo.name}</span>
                  {activeRepo.branch && <code className="scm-branch-name">{activeRepo.branch}</code>}
                </div>
              ) : null}
            </div>
          )}

          {/* 搜索/筛选框：VS Code 风格紧凑输入框 */}
          <div className="file-filter-box scm-filter-box">
            <IconSearch />
            <input
              className="text-input file-filter scm-filter-input"
              aria-label="筛选文件名"
              value={view.query}
              placeholder={mode === "files" ? "筛选已展开的文件…" : "筛选更改…"}
              onChange={(event) => store?.setQuery(event.target.value)}
            />
            {view.query && (
              <button
                type="button"
                className="scm-filter-clear"
                title="清空筛选"
                aria-label="清空筛选"
                onClick={() => store?.setQuery("")}
              >
                ×
              </button>
            )}
          </div>

          {mode === "files" && (
            <label className="file-hidden">
              <input
                type="checkbox"
                checked={view.showHidden}
                onChange={(event) => store?.setHidden(event.target.checked)}
              />
              <span>显示隐藏文件</span>
            </label>
          )}

          {mode === "git" && changes.value?.isRepo && (
            <div className="workspace-git-summary scm-stat-bar">
              <span>
                <b className="stat-staged">{summary.staged}</b> 已暂存
              </span>
              <span>
                <b className="stat-unstaged">{summary.unstaged}</b> 更改
              </span>
              <span>
                <b className="stat-untracked">{summary.untracked}</b> 未跟踪
              </span>
            </div>
          )}

          {/* 改动列表/树容器 */}
          <div
            className="workspace-file-list scm-resource-list"
            aria-label={mode === "files" ? "工作区文件树" : "Git 改动列表"}
          >
            {mode === "files" ? (
              <Directory key={store?.key} path="" depth={0} state={state} onOpen={open} />
            ) : changes.loading ? (
              <div className="workspace-list-state" role="status">
                <span className="file-state-spinner" />
                读取 Git 状态…
              </div>
            ) : changes.error && !changes.value ? (
              <p className="file-error" role="alert">
                {changes.error}
              </p>
            ) : !changes.value?.isRepo ? (
              <div className="workspace-sidebar-empty scm-empty-guide">
                <IconGit />
                <p>非 Git 仓库</p>
                <small>当前工作区未检测到 Git 仓库。</small>
                <div className="scm-pipi-toml-hint">
                  <span>在项目根目录的 <code>pipi.toml</code> 中声明子仓库：</span>
                  <pre>
                    {`[git]\nrepositories = [\n  "subproject-a",\n  "packages/subproject-b"\n]`}
                  </pre>
                </div>
              </div>
            ) : !changes.value.files.length ? (
              <div className="workspace-sidebar-empty scm-clean-state">
                <IconGit />
                <p>没有未提交的更改</p>
                <small>工作区干净 (Working tree clean)。</small>
              </div>
            ) : !changeFiles.length ? (
              <p className="file-notice">没有匹配的改动文件。</p>
            ) : (
              groups
                .filter((group) => group.files.length > 0)
                .map((group) => {
                  const isCollapsed = Boolean(collapsedGroups[group.id]);
                  const treeNodes =
                    gitViewMode === "tree" ? buildGitTree(group.files) : [];
                  return (
                    <section key={group.id} className={`workspace-git-group scm-group ${group.tone}`}>
                      <button
                        type="button"
                        className="scm-group-header"
                        aria-expanded={!isCollapsed}
                        onClick={() => toggleGroup(group.id)}
                      >
                        <IconCaret expanded={!isCollapsed} />
                        <span className="scm-group-title">{group.name}</span>
                        <span className="scm-count-pill">{group.files.length}</span>
                      </button>
                      {!isCollapsed && (
                        <div className="scm-group-content">
                          {gitViewMode === "list"
                            ? group.files.map((file) => (
                                <GitRow
                                  key={file.path}
                                  file={file}
                                  active={file.path === view.selected}
                                  multiRepo={multiRepo && selectedRepo === "all"}
                                  copied={copiedPath === file.path && clipboard.state === "copied"}
                                  onCopy={handleCopy}
                                  onOpen={open}
                                />
                              ))
                            : (
                                <GitTree
                                  nodes={treeNodes}
                                  depth={0}
                                  selected={view.selected}
                                  collapsedFolders={collapsedFolders}
                                  copiedPath={clipboard.state === "copied" ? copiedPath : null}
                                  onCopy={handleCopy}
                                  onToggleFolder={toggleFolder}
                                  onOpen={open}
                                  multiRepo={multiRepo && selectedRepo === "all"}
                                />
                              )}
                        </div>
                      )}
                    </section>
                  );
                })
            )}
          </div>

          {mode === "git" && changes.error && changes.value && <p className="file-error" role="status">刷新失败，显示上次结果：{changes.error}</p>}

          {mode === "files" ? (
            <form
              className="file-open-path"
              onSubmit={(event) => {
                event.preventDefault();
                if (path) open(path);
              }}
            >
              <input
                className="text-input"
                aria-label="打开工作区文件路径"
                placeholder="按路径打开文件…"
                value={path}
                onChange={(event) => setPath(event.target.value)}
              />
              <button type="submit" className="btn small" disabled={!path}>
                打开
              </button>
            </form>
          ) : (
            <div className="scm-footer">
              <span className="scm-footer-base"><IconGitBranch /> HEAD <span>↔</span> 工作区</span>
              <p className="file-sidebar-hint">已暂存与未暂存差异合并展示</p>
              {clipboard.state === "failed" && <p className="file-error" role="status">{clipboard.label}</p>}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function summarize(files: WorkspaceChange[]) {
  let staged = 0;
  let unstaged = 0;
  let untracked = 0;
  for (const file of files) {
    const meta = gitStatusMeta(file.status);
    if (meta.tone === "untracked") untracked += 1;
    else {
      if (meta.staged) staged += 1;
      // 同时含暂存/工作区更改的文件归入暂存组，摘要与列表保持同一口径。
      if (meta.unstaged && !meta.staged) unstaged += 1;
    }
  }
  return { staged, unstaged, untracked, total: files.length };
}

function GitRow({
  file,
  active,
  multiRepo,
  copied,
  onCopy,
  onOpen,
}: {
  file: WorkspaceChange;
  active: boolean;
  multiRepo: boolean;
  copied: boolean;
  onCopy: (e: React.MouseEvent, path: string) => void;
  onOpen: (path: string) => void;
}) {
  const meta = gitStatusMeta(file.status);
  const split = file.path.lastIndexOf("/");
  const dir = split < 0 ? "" : file.path.slice(0, split + 1);
  const name = split < 0 ? file.path : file.path.slice(split + 1);
  return (
    <div
      className={`scm-row scm-file-row${active ? " active" : ""}`}
      title={file.path}
    >
      <button type="button" className="scm-row-open" aria-pressed={active} onClick={() => onOpen(file.path)}>
        <span className="scm-row-icon">
          <FileGlyph path={file.path} size="sm" />
        </span>
        <span className={`scm-row-label ${meta.tone}`}>
          <span className="scm-filename">{name}</span>
          {dir && <span className="scm-dirname">{dir}</span>}
        </span>
        {multiRepo && file.repository && (
          <span className="scm-repo-tag" title={`仓库：${file.repository}`}>
            {file.repository}
          </span>
        )}
      </button>
      <div className="scm-row-actions">
        <button
          type="button"
          className="scm-action-btn"
          title={copied ? "已复制路径" : "复制路径"}
          aria-label="复制路径"
          onClick={(e) => onCopy(e, file.path)}
        >
          {copied ? <IconCheck /> : <IconCopy />}
        </button>
      </div>
      <span
        className={`scm-status-badge ${meta.tone}`}
        title={`${meta.label} (${meta.code.trim() || "?"})`}
      >
        {meta.code.trim() || "?"}
      </span>
    </div>
  );
}

interface GitTreeNode {
  name: string;
  path: string;
  isDir: boolean;
  file?: WorkspaceChange;
  children: GitTreeNode[];
}

function buildGitTree(files: WorkspaceChange[]): GitTreeNode[] {
  interface MutableNode {
    name: string;
    path: string;
    isDir: boolean;
    file?: WorkspaceChange;
    children: Map<string, MutableNode>;
  }

  const root: MutableNode = {
    name: "",
    path: "",
    isDir: true,
    children: new Map(),
  };

  for (const file of files) {
    const parts = file.path.split("/");
    let current = root;
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      const isLast = i === parts.length - 1;
      const currentPath = parts.slice(0, i + 1).join("/");
      if (isLast) {
        current.children.set(part, {
          name: part,
          path: file.path,
          isDir: false,
          file,
          children: new Map(),
        });
      } else {
        if (!current.children.has(part)) {
          current.children.set(part, {
            name: part,
            path: currentPath,
            isDir: true,
            children: new Map(),
          });
        }
        current = current.children.get(part)!;
      }
    }
  }

  function convert(node: MutableNode): GitTreeNode[] {
    const result: GitTreeNode[] = [];
    const entries = Array.from(node.children.values()).sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    for (const entry of entries) {
      result.push({
        name: entry.name,
        path: entry.path,
        isDir: entry.isDir,
        file: entry.file,
        children: convert(entry),
      });
    }
    return result;
  }

  return convert(root);
}

function collectFolderPaths(nodes: GitTreeNode[], set: Set<string>) {
  for (const node of nodes) {
    if (node.isDir) {
      set.add(node.path);
      collectFolderPaths(node.children, set);
    }
  }
}

function countTreeFiles(node: GitTreeNode): number {
  if (!node.isDir) return 1;
  return node.children.reduce((sum, child) => sum + countTreeFiles(child), 0);
}

function GitTree({
  nodes,
  depth,
  selected,
  collapsedFolders,
  copiedPath,
  onCopy,
  onToggleFolder,
  onOpen,
  multiRepo,
}: {
  nodes: GitTreeNode[];
  depth: number;
  selected: string | null;
  collapsedFolders: Set<string>;
  copiedPath: string | null;
  onCopy: (e: React.MouseEvent, path: string) => void;
  onToggleFolder: (path: string) => void;
  onOpen: (path: string) => void;
  multiRepo: boolean;
}) {
  return (
    <>
      {nodes.map((node) => {
        if (node.isDir) {
          const isCollapsed = collapsedFolders.has(node.path);
          const count = countTreeFiles(node);
          return (
            <div key={node.path} className="scm-tree-node">
              <button
                type="button"
                className="scm-row scm-tree-row dir"
                style={{ paddingLeft: 8 + depth * 14 }}
                aria-expanded={!isCollapsed}
                onClick={() => onToggleFolder(node.path)}
              >
                <span className="scm-twistie">
                  <IconCaret expanded={!isCollapsed} />
                </span>
                <span className="scm-row-icon folder">
                  <IconFolder />
                </span>
                <span className="scm-row-label dir-label">{node.name}</span>
                <span className="scm-count-pill">{count}</span>
              </button>
              {!isCollapsed && (
                <div className="scm-tree-branch">
                  <GitTree
                    nodes={node.children}
                    depth={depth + 1}
                    selected={selected}
                    collapsedFolders={collapsedFolders}
                    copiedPath={copiedPath}
                    onCopy={onCopy}
                    onToggleFolder={onToggleFolder}
                    onOpen={onOpen}
                    multiRepo={multiRepo}
                  />
                </div>
              )}
            </div>
          );
        }

        const file = node.file!;
        const meta = gitStatusMeta(file.status);
        const isActive = file.path === selected;

        return (
          <div
            key={file.path}
            className={`scm-row scm-tree-row file${isActive ? " active" : ""}`}
            style={{ paddingLeft: 8 + depth * 14 + 14 }}
            title={file.path}
          >
            <button type="button" className="scm-row-open" aria-pressed={isActive} onClick={() => onOpen(file.path)}>
              <span className="scm-row-icon">
                <FileGlyph path={file.path} size="sm" />
              </span>
              <span className={`scm-row-label ${meta.tone}`}>
                <span className="scm-filename">{node.name}</span>
              </span>
              {multiRepo && file.repository && (
                <span className="scm-repo-tag" title={`仓库：${file.repository}`}>
                  {file.repository}
                </span>
              )}
            </button>
            <div className="scm-row-actions">
              <button
                type="button"
                className="scm-action-btn"
                title={copiedPath === file.path ? "已复制路径" : "复制路径"}
                aria-label="复制路径"
                onClick={(e) => onCopy(e, file.path)}
              >
                {copiedPath === file.path ? <IconCheck /> : <IconCopy />}
              </button>
            </div>
            <span
              className={`scm-status-badge ${meta.tone}`}
              title={`${meta.label} (${meta.code.trim() || "?"})`}
            >
              {meta.code.trim() || "?"}
            </span>
          </div>
        );
      })}
    </>
  );
}

function Directory({
  path,
  depth,
  state,
  onOpen,
}: {
  path: string;
  depth: number;
  state: WorkspaceFilesState;
  onOpen: (path: string) => void;
}) {
  const { target, store, view } = state;
  const lookupTarget = target?.workspace || target?.name || "";
  const nested = useFileResource<WorkspaceListing>(
    path && state.active && store ? JSON.stringify(["listing", store.key, path]) : null,
    state.revision,
    () => invoke("list_workspace_files", { agentName: lookupTarget, path })
  );
  const resource = path ? nested : state.rootListing;
  if (resource.loading)
    return (
      <div className="workspace-list-state" role="status">
        <span className="file-state-spinner" />
        读取目录…
      </div>
    );
  if (resource.error && !resource.value)
    return (
      <p className="file-error" role="alert">
        {resource.error}
      </p>
    );
  const entries =
    resource.value?.entries.filter(
      (item) =>
        (view.showHidden || !item.name.startsWith(".")) &&
        (item.kind === "directory" ||
          item.name.toLowerCase().includes(view.query.toLowerCase()))
    ) ?? [];
  return (
    <>
      {resource.error && resource.value && <p className="file-error" role="status">刷新失败，显示上次目录：{resource.error}</p>}
      {!entries.length && (
        <p className="file-notice">{view.query ? "没有匹配的文件。" : "空目录"}</p>
      )}
      {entries.map((item) => {
        const directory = item.kind === "directory";
        const expanded = directory && view.expanded.includes(item.path);
        return (
          <div
            key={item.path}
            className={depth > 0 ? "file-tree-node nested" : "file-tree-node"}
          >
            <button
              type="button"
              className={`file-row${!directory && item.path === view.selected ? " active" : ""}`}
              style={{ paddingLeft: 6 + depth * 15 }}
              title={
                item.kind === "symlink"
                  ? `${item.path}（符号链接不允许预览）`
                  : item.path
              }
              aria-expanded={directory ? expanded : undefined}
              aria-pressed={directory ? undefined : item.path === view.selected}
              disabled={!["directory", "file"].includes(item.kind)}
              onClick={() =>
                directory ? store?.toggleFolder(item.path) : onOpen(item.path)
              }
            >
              {directory ? (
                <>
                  <IconCaret expanded={expanded} />
                  <span className="file-row-icon folder">
                    <IconFolder />
                  </span>
                </>
              ) : (
                <>
                  <span className="file-caret-spacer" />
                  <FileGlyph path={item.path} size="sm" />
                </>
              )}
              <span className="file-row-name">{item.name}</span>
              {!directory && <small>{formatSize(item.size)}</small>}
            </button>
            {expanded && (
              <Directory
                path={item.path}
                depth={depth + 1}
                state={state}
                onOpen={onOpen}
              />
            )}
          </div>
        );
      })}
      {resource.value?.truncated && (
        <p className="file-notice">此目录超过 2000 项，仅显示部分内容。</p>
      )}
    </>
  );
}
