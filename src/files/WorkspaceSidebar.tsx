import { useContext, useMemo, useState } from "react";
import { LayoutContext } from "../app/layout-context";
import { ChoiceSelect } from "../Select";
import { getWorkspaceBasename, type WorkspaceInfo } from "../app/workspaces";
import {
  IconCaret,
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
import { gitStatusMeta, summarizeChanges } from "./diff";
import { FileGlyph, FolderGlyph } from "./FileGlyph";
import { formatSize } from "./FilePreview";
import { useFileResource, type WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceChange, WorkspaceListing } from "./types";

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
  const [selectedRepo, setSelectedRepo] = useState<string>("all");
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(new Set());

  const { target, mode, store, view, changes } = state;
  const open = (filePath: string) => {
    store?.open(filePath);
    closeNavigation();
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

  const summary = summarizeChanges(repoFilteredFiles);

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
      {/* 搜索置顶：所有分区的过滤框都作为侧栏第一行，滚动时保持可见。 */}
      {target && (
        <SidebarSearch
          value={view.query}
          onChange={(value) => store?.setQuery(value)}
          placeholder={mode === "files" ? t("files.filterFiles") : t("files.filterChanges")}
          label={t("files.filterFileName")}
          clearLabel={t("files.clearFilter")}
        />
      )}

      {/* 顶部标题栏：左侧工作区切换，右侧操作图标。 */}
      <div className="file-workbench-header scm-header-bar">
        <div className="workspace-sidebar-title scm-title">
          {workspaces.length > 0 ? (
            <ChoiceSelect
              className="workspace-header-select"
              ariaLabel={t("files.switchWorkspace")}
              value={currentWorkspacePath}
              choices={workspaces.map(w => ({ value: w.path, label: w.name || getWorkspaceBasename(w.path) }))}
              onChange={value => onSelectWorkspace?.(value)}
              placeholder={t("files.selectWorkspace")}
              searchable={false}
              menuInPortal
              menuPlacement="auto"
            />
          ) : (
            <strong className="truncate">{currentWorkspaceName}</strong>
          )}
        </div>
        <div className="workspace-sidebar-actions scm-header-actions">
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
          {mode === "git" && multiRepo && (
            <div className="scm-repo-bar">
              <div className="scm-repo-picker">
                <span className="scm-repo-picker-label">
                  <IconGitBranch /> {t("files.repository")}
                </span>
                <ChoiceSelect
                  className="workspace-picker-select"
                  ariaLabel={t("files.selectRepository")}
                  value={selectedRepo}
                  choices={[{ value: "all", label: t("files.allRepositories", { count: repositories.length }) },
                    ...repositories.map(r => ({ value: r.name, label: `${r.name}${r.branch ? ` (${r.branch})` : ""}` }))]}
                  onChange={setSelectedRepo}
                  searchable={false} menuInPortal menuPlacement="auto"
                />
              </div>
            </div>
          )}

          {mode === "git" && changes.value?.isRepo && (
            <div className="workspace-git-summary scm-stat-bar">
              <span>
                <b className="stat-staged">{summary.staged}</b> {t("files.staged")}
              </span>
              <span>
                <b className="stat-unstaged">{summary.unstaged}</b> {t("files.changes")}
              </span>
              <span>
                <b className="stat-untracked">{summary.untracked}</b> {t("files.untracked")}
              </span>
            </div>
          )}

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
            ) : !changes.value.files.length ? (
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
                                  multiRepo={multiRepo && selectedRepo === "all"}
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

function GitRow({
  file,
  active,
  multiRepo,
  onOpen,
}: {
  file: WorkspaceChange;
  active: boolean;
  multiRepo: boolean;
  onOpen: (path: string) => void;
}) {
  const { t, locale } = useI18n();
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
        {multiRepo && file.repository && (
          <span className="truncate scm-repo-tag" title={t("files.repositoryTitle", { name: file.repository })}>
            {file.repository}
          </span>
        )}
      </button>
      <LineCount file={file} />
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

function LineCount({ file }: { file: WorkspaceChange }) {
  return <LineDiff additions={file.additions} deletions={file.deletions} />;
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
  multiRepo,
}: {
  nodes: GitTreeNode[];
  depth: number;
  selected: string | null;
  collapsedFolders: Set<string>;
  onToggleFolder: (path: string) => void;
  onOpen: (path: string) => void;
  multiRepo: boolean;
}) {
  const { t, locale } = useI18n();
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
                    multiRepo={multiRepo}
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
              {multiRepo && file.repository && (
                <span className="truncate scm-repo-tag" title={t("files.repositoryTitle", { name: file.repository })}>
                  {file.repository}
                </span>
              )}
            </button>
            <LineCount file={file} />
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
