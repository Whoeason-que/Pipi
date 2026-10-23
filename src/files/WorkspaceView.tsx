import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { getWorkspaceBasename } from "../app/workspaces";
import type { SessionSummaryView } from "../types";
import { FilePreview, type PreviewSource } from "./FilePreview";
import type { WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceDiff, WorkspaceFile } from "./types";

export function WorkspaceView({ state, sessions, defaultSession, onOpenFile, onReference }: {
  state: WorkspaceFilesState;
  sessions: SessionSummaryView[];
  defaultSession: string | null;
  onOpenFile: (path: string) => void;
  onReference: (text: string, sessionId: string | null) => void;
}) {
  const { mode, target, view, store, preview } = state;
  const [selectedSession, setSelectedSession] = useState<string | null>(defaultSession);
  const validTarget = selectedSession === null || sessions.some(session => session.id === selectedSession);
  const targetTitle = selectedSession === null ? "新会话草稿" : sessions.find(session => session.id === selectedSession)?.title || selectedSession;
  const root = (mode === "files" ? state.rootListing.value?.workspace : state.changes.value?.workspace) ?? target?.workspace ?? "默认工作区";
  const workspaceDisplayName = target?.name || getWorkspaceBasename(root);
  const source: PreviewSource | null = !preview.value ? null : mode === "files" ? { type: "file", file: preview.value as WorkspaceFile } : { type: "git", diff: preview.value as WorkspaceDiff };
  const openRef = useRef(onOpenFile);
  openRef.current = onOpenFile;
  const openFile = useCallback((path: string) => openRef.current(path), []);
  const tabs = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { tabs.current?.querySelector('[aria-pressed="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" }); }, [view.selected]);
  const remember = useCallback((top: number) => { if (view.selected) store?.rememberScroll(view.selected, top); }, [store, view.selected]);
  return <div className="file-workbench workspace-files-page">
    <header className="workspace-files-toolbar">
      <NavigationToggle />
      <strong>{mode === "files" ? "File" : "Git"}</strong>
      <span className="workspace-toolbar-name">{workspaceDisplayName}</span>
      <code title={root}>{target ? root : "请选择工作区"}</code>
      <button type="button" className="btn small ghost" onClick={state.refresh} disabled={!target}>刷新</button>
    </header>
    {target && mode === "git" && state.changes.value?.isRepo === false
      ? <div className="file-preview-empty"><strong>此工作区不属于 Git 仓库</strong><p>可以切换工作区，或进入 File 浏览文件。</p></div>
      : target ? <FilePreview path={view.selected} source={source} loading={preview.loading} error={preview.error} onOpenFile={openFile}
      scroll={view.selected ? store?.scroll(view.selected) : 0} onScroll={remember}
      tabs={<div className="file-preview-tabs" ref={tabs} role="group" aria-label="已打开文件">{view.tabs.map(path => <div className={view.selected === path ? "active" : ""} key={path}>
        <button type="button" title={path} aria-pressed={view.selected === path} onClick={() => store?.select(path)}>{mode === "git" ? "Δ " : ""}{path.split("/").pop()}</button>
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
      </label> : undefined} /> : <div className="file-preview-empty"><strong>暂无可预览的工作区</strong><p>请先在工作区列表创建或选择一个工作区。</p></div>}
  </div>;
}
