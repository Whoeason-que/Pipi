import { useContext, useState } from "react";
import { DropdownMenu } from "radix-ui";
import { LayoutContext } from "../app/layout-context";
import { WorkspaceScopeBar, ScopeRadioMenu } from "../app/WorkspaceScopeBar";
import { getWorkspaceBasename, type WorkspaceInfo } from "../app/workspaces";
import {
  IconCaret,
  IconChevronDown,
  IconCollapseAll,
  IconEye,
  IconEyeOff,
  IconFolder,
  IconGit,
  IconGitBranch,
  IconList,
  IconRefresh,
  IconTree,
} from "../icons";
import { SidebarSearch } from "../components/SidebarSearch";
import { useI18n } from "../i18n";
import { invoke } from "../platform";
import { gitStatusMeta } from "./diff";
import { FileGlyph, FolderGlyph } from "./FileGlyph";
import { formatSize } from "./FilePreview";
import { useFileResource, type WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceChange, WorkspaceListing, WorkspaceRepoInfo } from "./types";

export function WorkspaceSidebar({
  state,
  workspaces = [],
  onSelectWorkspace,
}: {
  state: WorkspaceFilesState;
  workspaces?: WorkspaceInfo[];
  onSelectWorkspace?: (path: string) => void;
}) {
  const { closeNavigation } = useContext(LayoutContext);
  const { t } = useI18n();
  const [path, setPath] = useState("");
  const [gitViewMode, setGitViewMode] = useState<"list" | "tree">("list");
  const [selectedRepo, setSelectedRepo] = useState<string | null>(null);
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(new Set());

  const { target, mode, store, view, changes } = state;
  const open = (filePath: string) => {
    store?.open(filePath);
    closeNavigation();
  };

  // 一个工作区可能声明多个子仓库（pipi.toml）；标题栏左侧只选一个，不提供「全部」。
  const repositories = changes.value?.repositories ?? [];
  const repo = repositories.find((item) => item.name === selectedRepo) ?? repositories[0];
  const repoFiles = (changes.value?.files ?? []).filter(
    (file) => !repo || repositories.length < 2 || !file.repository || file.repository === repo.name
  );

  const changeFiles = repoFiles.filter((file) =>
    file.path.toLowerCase().includes(view.query.toLowerCase())
  );

  const groups = [
    {
      id: "staged",
      name: t("files.stagedChanges"),
      tone: "staged" as const,
      files: changeFiles.filter((file) => gitStatusMeta(file.status).staged),
    },
    {
      id: "unstaged",
      name: t("files.changes"),
      tone: "unstaged" as const,
      files: changeFiles.filter((file) => {
        const meta = gitStatusMeta(file.status);
        return meta.tone !== "untracked" && meta.unstaged && !meta.staged;
      }),
    },
    {
      id: "untracked",
      name: t("files.untrackedFiles"),
      tone: "untracked" as const,
      files: changeFiles.filter(
        (file) => gitStatusMeta(file.status).tone === "untracked"
      ),
    },
  ];

  const currentWorkspacePath = target?.workspace || "";
  const currentWorkspaceName = target?.name || getWorkspaceBasename(currentWorkspacePath);

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

  return (
    <div className={`file-workbench workspace-files-sidebar${mode === "git" ? " vscode-scm-sidebar" : ""}`}>
      {/* C 行：工作区 pill + 过滤框（工作区切换与搜索合并为一行，搜索置顶不变）。 */}
      <WorkspaceScopeBar
        workspaces={workspaces}
        value={currentWorkspacePath}
        onSelect={(value) => onSelectWorkspace?.(value)}
        ariaLabel={t("files.switchWorkspace")}
        placeholder={currentWorkspaceName || t("files.selectWorkspace")}
      >
        {target && (
          <SidebarSearch
            bare
            value={view.query}
            onChange={(value) => store?.setQuery(value)}
            placeholder={mode === "files" ? t("files.filterFiles") : t("files.filterChanges")}
            label={t("files.filterFileName")}
            clearLabel={t("files.clearFilter")}
          />
        )}
      </WorkspaceScopeBar>

      {/* 顶部操作栏：Files 左侧是工作区名 + 绝对路径，Git 左侧是当前仓库；右侧是视图操作。 */}
      <div className="file-workbench-header">
        {mode === "files" && target && (
          <div className="scm-file-identity" title={currentWorkspacePath}>
            <span className="truncate scm-file-identity-path">{currentWorkspacePath}</span>
          </div>
        )}
        {mode === "git" && repo && (
          <RepoPicker repositories={repositories} value={repo.name} onSelect={setSelectedRepo} />
        )}
        <div className="scm-header-actions">
          {mode === "git" && (
            <>
              <button
                type="button"
                className={`icon-btn small${gitViewMode === "tree" ? " active" : ""}`}
                title={t("files.viewAsTree")}
                aria-label={t("files.treeView")}
                aria-pressed={gitViewMode === "tree"}
                onClick={() => setGitViewMode("tree")}
              >
                <IconTree />
              </button>
              <button
                type="button"
                className={`icon-btn small${gitViewMode === "list" ? " active" : ""}`}
                title={t("files.viewAsList")}
                aria-label={t("files.listView")}
                aria-pressed={gitViewMode === "list"}
                onClick={() => setGitViewMode("list")}
              >
                <IconList />
              </button>
              <button
                type="button"
                className="icon-btn small"
                title={t("files.collapseAll")}
                aria-label={t("files.collapseAll")}
                onClick={handleCollapseAll}
              >
                <IconCollapseAll />
              </button>
            </>
          )}
          {mode === "files" && (
            <button
              type="button"
              className={`icon-btn small${view.showHidden ? " active" : ""}`}
              title={t("files.showHiddenFiles")}
              aria-label={t("files.showHiddenFiles")}
              aria-pressed={view.showHidden}
              onClick={() => store?.setHidden(!view.showHidden)}
            >
              {view.showHidden ? <IconEye /> : <IconEyeOff />}
            </button>
          )}
          <button
            type="button"
            className="icon-btn small"
            title={t("common.refresh")}
            aria-label={t("common.refresh")}
            onClick={state.refresh}
            disabled={!target}
          >
            <IconRefresh />
          </button>
        </div>
      </div>

      {!target ? (
        <div className="empty-state workspace-sidebar-empty">
          <IconFolder />
          <p>{t("files.noWorkspace")}</p>
          <small>{t("files.noWorkspaceHintSidebar")}</small>
        </div>
      ) : (
        <>
          {/* 改动列表/树容器 */}
          <div
            className="workspace-file-list scm-resource-list"
            aria-label={mode === "files" ? t("files.workspaceFileTree") : t("files.gitChangesList")}
          >
            {mode === "files" ? (
              <Directory key={store?.key} path="" depth={0} state={state} onOpen={open} />
            ) : changes.loading ? (
              <div className="workspace-list-state" role="status">
                <span className="file-state-spinner" />
                {t("files.readingGitStatus")}
              </div>
            ) : changes.error && !changes.value ? (
              <p className="file-error" role="alert">
                {changes.error}
              </p>
            ) : !changes.value?.isRepo ? (
              <div className="empty-state workspace-sidebar-empty scm-empty-guide">
                <IconGit />
                <p>{t("files.notGitRepo")}</p>
                <small>{t("files.notGitRepoDetected")}</small>
                <div className="scm-pipi-toml-hint">
                  <span>{t("files.declareSubreposPrefix")} <code>pipi.toml</code> {t("files.declareSubreposSuffix")}</span>
                  <pre>
                    {`[git]\nrepositories = [\n  "subproject-a",\n  "packages/subproject-b"\n]`}
                  </pre>
                </div>
              </div>
            ) : !repoFiles.length ? (
              <div className="empty-state workspace-sidebar-empty scm-clean-state">
                <IconGit />
                <p>{t("files.noUncommittedChanges")}</p>
                <small>{t("files.workingTreeClean")}</small>
              </div>
            ) : !changeFiles.length ? (
              <p className="file-notice">{t("files.noMatchingChanges")}</p>
            ) : (
              groups
                .filter((group) => group.files.length > 0)
                .map((group) => {
                  const isCollapsed = Boolean(collapsedGroups[group.id]);
                  const treeNodes =
                    gitViewMode === "tree" ? buildGitTree(group.files) : [];
                  const lineSum = sumLines(group.files);
                  return (
                    <section key={group.id} className={`workspace-git-group scm-group ${group.tone}`}>
                      <button
                        type="button"
                        className="scm-group-header"
                        aria-expanded={!isCollapsed}
                        onClick={() => toggleGroup(group.id)}
                      >
                        <IconCaret expanded={!isCollapsed} />
                        <span className="truncate scm-group-title">{group.name}</span>
                        <LineDiff additions={lineSum.additions} deletions={lineSum.deletions} />
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
                                  onOpen={open}
                                />
                              ))
                            : (
                                <GitTree
                                  nodes={treeNodes}
                                  depth={0}
                                  selected={view.selected}
                                  collapsedFolders={collapsedFolders}
                                  onToggleFolder={toggleFolder}
                                  onOpen={open}
                                />
                              )}
                        </div>
                      )}
                    </section>
                  );
                })
            )}
          </div>

          {mode === "git" && changes.error && changes.value && <p className="file-error" role="status">{t("files.refreshFailed", { error: changes.error })}</p>}

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
                aria-label={t("files.openByPath")}
                placeholder={t("files.openByPathPlaceholder")}
                value={path}
                onChange={(event) => setPath(event.target.value)}
              />
              <button type="submit" className="btn small" disabled={!path}>
                {t("files.open")}
              </button>
            </form>
          ) : (
            <div className="scm-footer">
              <span className="scm-footer-base"><IconGitBranch /> HEAD <span>↔</span> {t("files.workspace")}</span>
              <p className="caption file-sidebar-hint">{t("files.mergedDiffHint")}</p>
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** 标题栏左侧的仓库选择器：只显示当前仓库，多仓库时点开切换（不提供「全部」）。 */
function RepoPicker({
  repositories,
  value,
  onSelect,
}: {
  repositories: WorkspaceRepoInfo[];
  value: string;
  onSelect: (name: string) => void;
}) {
  const { t } = useI18n();
  const current = repositories.find((repo) => repo.name === value) ?? repositories[0];
  if (!current) return null;
  const title = current.branch ? `${current.name} (${current.branch})` : current.name;
  const label = (
    <>
      <IconGitBranch />
      <span className="truncate scm-repo-trigger-name">{current.name}</span>
    </>
  );
  if (repositories.length < 2) {
    return (
      <div className="scm-repo-picker">
        <div className="scm-repo-trigger static" title={title}>{label}</div>
      </div>
    );
  }
  return (
    <div className="scm-repo-picker">
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button type="button" className="scm-repo-trigger" aria-label={t("files.selectRepository")} title={title}>
            {label}
            <span className="scm-repo-trigger-chevron"><IconChevronDown /></span>
          </button>
        </DropdownMenu.Trigger>
        <ScopeRadioMenu
          options={repositories.map((repo) => ({
            value: repo.name,
            label: repo.name,
            description: repo.branch || repo.path,
            title: repo.path,
          }))}
          value={current.name}
          onSelect={onSelect}
          ariaLabel={t("files.selectRepository")}
        />
      </DropdownMenu.Root>
    </div>
  );
}

function GitRow({
  file,
  active,
  onOpen,
}: {
  file: WorkspaceChange;
  active: boolean;
  onOpen: (path: string) => void;
}) {
  const { locale } = useI18n();
  const meta = gitStatusMeta(file.status, locale);
  const split = file.path.lastIndexOf("/");
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
          <span className="truncate scm-filename">{name}</span>
        </span>
      </button>
      <LineDiff additions={file.additions} deletions={file.deletions} />
      <span
        className={`scm-status-badge ${meta.tone}`}
        title={`${meta.label} (${meta.code.trim() || "?"})`}
      >
        {meta.code.trim() || "?"}
      </span>
    </div>
  );
}

/** 增删行数：只显示非零的一侧；没有统计（如未跟踪）时不显示。 */
function LineDiff({ additions, deletions }: { additions?: number; deletions?: number }) {
  if (typeof additions !== "number" || typeof deletions !== "number") return null;
  if (additions === 0 && deletions === 0) return null;
  return (
    <span className="scm-line-count" title={`+${additions} −${deletions}`}>
      {additions > 0 && <span className="scm-line-add">+{additions}</span>}
      {deletions > 0 && <span className="scm-line-del">−{deletions}</span>}
    </span>
  );
}

/** 分组标题的增删行数：只累加有统计的文件；一个都没有时返回空。 */
function sumLines(files: WorkspaceChange[]) {
  let additions = 0;
  let deletions = 0;
  let known = false;
  for (const file of files) {
    if (typeof file.additions !== "number" || typeof file.deletions !== "number") continue;
    known = true;
    additions += file.additions;
    deletions += file.deletions;
  }
  return known ? { additions, deletions } : {};
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
  onToggleFolder,
  onOpen,
}: {
  nodes: GitTreeNode[];
  depth: number;
  selected: string | null;
  collapsedFolders: Set<string>;
  onToggleFolder: (path: string) => void;
  onOpen: (path: string) => void;
}) {
  const { locale } = useI18n();
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
                  <FolderGlyph path={node.path} expanded={!isCollapsed} />
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
                    onToggleFolder={onToggleFolder}
                    onOpen={onOpen}
                  />
                </div>
              )}
            </div>
          );
        }

        const file = node.file!;
        const meta = gitStatusMeta(file.status, locale);
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
                <span className="truncate scm-filename">{node.name}</span>
              </span>
            </button>
            <LineDiff additions={file.additions} deletions={file.deletions} />
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
  const { t } = useI18n();
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
        {t("files.readingDirectory")}
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
      {resource.error && resource.value && <p className="file-error" role="status">{t("files.refreshFailed", { error: resource.error })}</p>}
      {!entries.length && (
        <p className="file-notice">{view.query ? t("files.noMatchingFiles") : t("files.emptyDirectory")}</p>
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
                  ? t("files.symlinkUnpreviewable", { path: item.path })
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
                    <FolderGlyph path={item.path} expanded={expanded} />
                  </span>
                </>
              ) : (
                <>
                  <span className="file-caret-spacer" />
                  <FileGlyph path={item.path} size="sm" />
                </>
              )}
              <span className="truncate file-row-name">{item.name}</span>
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
        <p className="file-notice">{t("files.directoryTruncated")}</p>
      )}
    </>
  );
}
