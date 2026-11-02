import { useCallback, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { useRightRail } from "../app/right-rail";
import { getWorkspaceBasename } from "../app/workspaces";
import { useI18n } from "../i18n";
import { IconFile, IconGit, IconPanelRight, IconRefresh } from "../icons";
import type { SessionSummaryView } from "../types";
import { FileGlyph } from "./FileGlyph";
import { FilePreview, type PreviewSource } from "./FilePreview";
import { WorkspaceRail } from "./WorkspaceRail";
import type { WorkspaceFilesState } from "./use-workspace-files";
import type { WorkspaceDiff, WorkspaceFile } from "./types";
import type { WorkspaceMode } from "./workspace-view";

export function WorkspaceView({ state, sessions, canReference, defaultSession, onOpenFile, onReference, onModeChange }: {
  state: WorkspaceFilesState;
  sessions: SessionSummaryView[];
  canReference: boolean;
  defaultSession: string | null;
  onOpenFile: (path: string) => void;
  onReference: (text: string, sessionId: string | null) => void;
  onModeChange?: (mode: WorkspaceMode) => void;
}) {
  const { mode, target, view, store, preview, changes, rootListing } = state;
  const { t } = useI18n();
  const [selectedSession, setSelectedSession] = useState<string | null>(defaultSession);
  const rail = useRightRail("workspace");
  const validTarget = selectedSession === null || sessions.some(session => session.id === selectedSession);
  const targetTitle = selectedSession === null ? t("files.newSessionDraft") : sessions.find(session => session.id === selectedSession)?.title || selectedSession;
  const root = (mode === "files" ? rootListing.value?.workspace : changes.value?.workspace) ?? target?.workspace ?? t("files.defaultWorkspace");
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
    <div className="content-pane workspace-main">
    <header className="workspace-files-toolbar">
      <NavigationToggle />
      <div className="workspace-mode-switch" role="tablist" aria-label={t("files.workspaceView")}>
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
        <span className="workspace-change-dot" /> {t("files.changeCount", { count: changeCount })}
      </span>}
      <button type="button" className={`icon-btn${rail.open ? " active" : ""}`} title={t("files.fileInfo")} aria-label={t("files.toggleFileInfo")}
        aria-pressed={rail.open} onClick={rail.toggle}>
        <IconPanelRight />
      </button>
      <button type="button" className="btn small ghost" onClick={state.refresh} disabled={!target}
        aria-busy={preview.refreshing || changes.refreshing || rootListing.refreshing}>
        <IconRefresh /> {t("common.refresh")}
      </button>
    </header>
    <div className="workspace-page-body">
    {target && mode === "git" && changes.value?.isRepo === false
      ? <div className="empty-state file-preview-empty workspace-state">
        <span className="icon-tile file-empty-icon"><IconGit /></span>
        <strong>{t("files.notAGitRepo")}</strong>
        <p>{t("files.initGitBefore")} <code>pipi.toml</code> {t("files.initGitAfter")}</p>
        <button type="button" className="btn" onClick={() => onModeChange?.("files")}><IconFile /> {t("files.browseFiles")}</button>
      </div>
      : target ? <FilePreview path={view.selected} source={source} loading={preview.loading} error={preview.error} onOpenFile={openFile}
      scroll={view.selected ? store?.scroll(view.selected) : 0} onScroll={remember}
      emptyTitle={mode === "git" ? t("files.selectChangedFile") : t("files.selectFileTitle")}
      emptyHint={mode === "git"
        ? t("files.changesEmptyHint")
        : t("files.filesEmptyHint")}
      tabs={<div className="file-preview-tabs" ref={tabs} role="group" aria-label={t("files.openFiles")}>{view.tabs.map(path => <div className={view.selected === path ? "active" : ""} key={path}>
        <button type="button" title={path} aria-pressed={view.selected === path} onClick={() => store?.select(path)}>
          <FileGlyph path={path} size="sm" />{mode === "git" && <span className="file-tab-diff" aria-hidden="true">Δ</span>}
          <span className="file-tab-name">{path.split("/").pop()}</span>
        </button>
        <button type="button" className="file-tab-close" aria-label={t("files.closeTab", { path })} onClick={() => store?.close(path)}>×</button>
      </div>)}</div>}
      onReference={mode === "files" ? text => onReference(text, selectedSession) : undefined}
      referenceDisabled={!validTarget || !canReference}
      referenceLabel={validTarget ? selectedSession === null ? t("files.addToNewDraft") : t("files.addToDraft", { title: targetTitle }) : t("files.reselectTarget")}
      referenceOptions={sessions.length > 0 ? <label className="workspace-reference-target">{t("files.referenceToSession")}
        <select className="text-input" aria-label={t("files.referenceTarget")} value={JSON.stringify(selectedSession)} onChange={event => setSelectedSession(JSON.parse(event.target.value))}>
          {!validTarget && <option value={JSON.stringify(selectedSession)}>{t("files.targetUnavailable")}</option>}
          <option value="null">{t("files.newSessionDraft")}</option>
          {sessions.map(session => <option key={session.id} value={JSON.stringify(session.id)}>{session.title || session.id}</option>)}
        </select>
      </label> : undefined} /> : <div className="empty-state file-preview-empty workspace-state">
        <span className="icon-tile file-empty-icon"><IconFile /></span>
        <strong>{t("files.noWorkspace")}</strong>
        <p>{t("files.noWorkspaceHint")}</p>
      </div>}
    </div>
    </div>
    <WorkspaceRail controller={rail} mode={mode} workspaceName={workspaceDisplayName} root={root}
      selectedPath={view.selected} source={source} changes={changes.value} loading={preview.loading}
      error={preview.error} />
  </div>;
}
