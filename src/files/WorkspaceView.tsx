import { useCallback, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { useRightRail } from "../app/right-rail";
import { getWorkspaceBasename } from "../app/workspaces";
import { IconFile, IconGit, IconPanelRight, IconRefresh } from "../icons";
import type { SessionSummaryView } from "../types";
import { FileGlyph } from "./FileGlyph";
import { FilePreview, type PreviewSource } from "./FilePreview";
import { WorkspaceRail } from "./WorkspaceRail";
import type { WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceDiff, WorkspaceFile } from "./types";
import type { WorkspaceMode } from "./workspace-view";

export function WorkspaceView({ state, sessions, defaultSession, onOpenFile, onReference, onModeChange }: {
  state: WorkspaceFilesState;
  sessions: SessionSummaryView[];
  defaultSession: string | null;
  onOpenFile: (path: string) => void;
  onReference: (text: string, sessionId: string | null) => void;
  onModeChange?: (mode: WorkspaceMode) => void;
}) {
  const { mode, target, view, store, preview, changes, rootListing } = state;
  const [selectedSession, setSelectedSession] = useState<string | null>(defaultSession);
  const rail = useRightRail("workspace");
  const validTarget = selectedSession === null || sessions.some(session => session.id === selectedSession);
  const targetTitle = selectedSession === null ? "新会话草稿" : sessions.find(session => session.id === selectedSession)?.title || selectedSession;
  const root = (mode === "files" ? rootListing.value?.workspace : changes.value?.workspace) ?? target?.workspace ?? "默认工作区";
  const workspaceDisplayName = target?.name || getWorkspaceBasename(root);
  const source: PreviewSource | null = !preview.value ? null : mode === "files" ? { type: "file", file: preview.value as WorkspaceFile } : { type: "git", diff: preview.value as WorkspaceDiff };
  const changeCount = changes.value?.files.length ?? 0;
  const openRef = useRef(onOpenFile);
  openRef.current = onOpenFile;
  const openFile = useCallback((path: string) => openRef.current(path), []);
  const tabs = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { tabs.current?.querySelector('[aria-pressed="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" }); }, [view.selected]);
  const remember = useCallback((top: number) => { if (view.selected) store?.rememberScroll(view.selected, top); }, [store, view.selected]);

  return <div className={`file-workbench workspace-files-page${mode === "git" ? " workspace-git-page" : ""}`} style={{ "--right-rail-width": `${rail.width}px` } as CSSProperties}>
    <div className="workspace-main">
    <header className="workspace-files-toolbar">
      <NavigationToggle />
      <div className="workspace-mode-switch" role="tablist" aria-label="工作区视图">
        <button type="button" role="tab" aria-selected={mode === "files"} className={mode === "files" ? "active" : ""}
          onClick={() => mode !== "files" && onModeChange?.("files")}><IconFile /> File</button>
        <button type="button" role="tab" aria-selected={mode === "git"} className={mode === "git" ? "active" : ""}
          onClick={() => mode !== "git" && onModeChange?.("git")}><IconGit /> Git{changeCount > 0 && <small>{changeCount}</small>}</button>
      </div>
      <div className="workspace-crumb" title={root}>
        <strong>{workspaceDisplayName}</strong>
        <code>{root}</code>
      </div>
      <span className="spacer" />
      {mode === "git" && changeCount > 0 && <span className="workspace-change-summary">
        <span className="workspace-change-dot" /> {changeCount} 个更改
      </span>}
      <button type="button" className={`icon-btn${rail.open ? " active" : ""}`} title="文件信息" aria-label="切换文件信息面板"
        aria-pressed={rail.open} onClick={rail.toggle}>
        <IconPanelRight />
      </button>
      <button type="button" className="btn small ghost" onClick={state.refresh} disabled={!target}
        aria-busy={preview.refreshing || changes.refreshing || rootListing.refreshing}>
        <IconRefresh /> 刷新
      </button>
    </header>
    <div className="workspace-page-body">
    {target && mode === "git" && changes.value?.isRepo === false
      ? <div className="file-preview-empty workspace-state">
        <span className="file-empty-icon"><IconGit /></span>
        <strong>此工作区不属于 Git 仓库</strong>
        <p>初始化 Git 或在项目根目录创建 <code>pipi.toml</code> 声明子仓库后，改动会显示在这里。</p>
        <button type="button" className="btn" onClick={() => onModeChange?.("files")}><IconFile /> 浏览文件</button>
      </div>
      : target ? <FilePreview path={view.selected} source={source} loading={preview.loading} error={preview.error} onOpenFile={openFile}
      scroll={view.selected ? store?.scroll(view.selected) : 0} onScroll={remember}
      emptyTitle={mode === "git" ? "选择一个改动文件" : "选择文件开始预览"}
      emptyHint={mode === "git"
        ? "左侧按「已暂存 / 未暂存 / 未跟踪」列出改动；打开文件后显示与比较基准的逐行差异。"
        : "在左侧文件树中选择文件，点击行号可以把选区引用到会话草稿。"}
      tabs={<div className="file-preview-tabs" ref={tabs} role="group" aria-label="已打开文件">{view.tabs.map(path => <div className={view.selected === path ? "active" : ""} key={path}>
        <button type="button" title={path} aria-pressed={view.selected === path} onClick={() => store?.select(path)}>
          <FileGlyph path={path} size="sm" />{mode === "git" && <span className="file-tab-diff" aria-hidden="true">Δ</span>}
          <span className="file-tab-name">{path.split("/").pop()}</span>
        </button>
        <button type="button" className="file-tab-close" aria-label={`关闭 ${path}`} onClick={() => store?.close(path)}>×</button>
      </div>)}</div>}
      onReference={mode === "files" ? text => onReference(text, selectedSession) : undefined}
      referenceDisabled={!validTarget || !sessions.length}
      referenceLabel={validTarget ? selectedSession === null ? "加入新会话草稿并返回会话" : `加入「${targetTitle}」草稿并返回会话` : "请重新选择目标会话"}
      referenceOptions={sessions.length > 0 ? <label className="workspace-reference-target">引用到会话
        <select className="text-input" aria-label="引用目标会话" value={JSON.stringify(selectedSession)} onChange={event => setSelectedSession(JSON.parse(event.target.value))}>
          {!validTarget && <option value={JSON.stringify(selectedSession)}>目标会话已不可用</option>}
          <option value="null">新会话草稿</option>
          {sessions.map(session => <option key={session.id} value={JSON.stringify(session.id)}>{session.title || session.id}</option>)}
        </select>
      </label> : undefined} /> : <div className="file-preview-empty workspace-state">
        <span className="file-empty-icon"><IconFile /></span>
        <strong>暂无可预览的工作区</strong>
        <p>请先在工作区列表创建或选择一个工作区。Agent 的工作区也可以直接在这里浏览。</p>
      </div>}
    </div>
    </div>
    <WorkspaceRail controller={rail} mode={mode} workspaceName={workspaceDisplayName} root={root}
      selectedPath={view.selected} source={source} changes={changes.value} loading={preview.loading}
      error={preview.error} />
  </div>;
}
