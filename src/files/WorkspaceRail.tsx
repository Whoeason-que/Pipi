// 工作区右栏：当前文件/Git 改动的元数据与工作区摘要。
// 预览与 diff 仍在主区，右栏不重复正文。

import { useMemo } from "react";
import { useRightRail, RightRail } from "../app/right-rail";
import { RightRailEmpty } from "../components/RightRailEmpty";
import { useCopyFeedback } from "../components/use-copy";
import { useI18n } from "../i18n";
import { IconCopy, IconFile, IconGit, IconFolder } from "../icons";
import type { WorkspaceChanges, WorkspaceDiff, WorkspaceFile } from "./types";
import { FileGlyph } from "./FileGlyph";
import { formatSize } from "./FilePreview";
import { gitStatusMeta, normalizePatch, parsePatch, summarizeChanges } from "./diff";

export function WorkspaceRail({
  controller,
  mode,
  workspaceName,
  root,
  selectedPath,
  source,
  changes,
  loading,
  error,
}: {
  controller: ReturnType<typeof useRightRail>;
  mode: "files" | "git";
  workspaceName: string;
  root: string;
  selectedPath: string | null;
  source: { type: "file"; file: WorkspaceFile } | { type: "git"; diff: WorkspaceDiff } | null;
  changes: WorkspaceChanges | null;
  loading: boolean;
  error: string;
}) {
  const clipboard = useCopyFeedback();
  const { t } = useI18n();
  const parsed = useMemo(() => source?.type === "git" ? parsePatch(normalizePatch(source.diff.patch)) : null, [source]);
  const status = selectedPath ? changes?.files.find((file) => file.path === selectedPath) : undefined;
  const summary = summarizeChanges(changes?.files ?? []);
  const title = mode === "files" ? t("files.fileInfo") : t("files.gitChanges");
  const label = source?.type === "file" ? source.file.path.split("/").pop() ?? source.file.path
    : source?.type === "git" ? source.diff.path.split("/").pop() ?? source.diff.path
      : null;

  return <RightRail
    controller={controller}
    id="workspace-rail"
    label={title}
    subtitle={workspaceName}
    icon={mode === "files" ? <IconFile /> : <IconGit />}
    actions={selectedPath ? <button type="button" className="icon-btn" title={clipboard.state === "idle" ? t("files.copyFilePath") : clipboard.state === "copied" ? t("common.copied") : t("chat.copyFailed")}
      aria-label={t("files.copyFilePath")} onClick={() => void clipboard.copy(selectedPath)}><IconCopy /></button> : undefined}
    tabs={[
      {
        id: "current",
        label: t("files.current"),
        content: <CurrentInfo label={label} selectedPath={selectedPath} source={source} status={status}
          loading={loading} error={error} parsed={parsed} workspaceName={workspaceName} mode={mode} />,
      },
      {
        id: "workspace",
        label: t("files.workspace"),
        content: <WorkspaceInfo mode={mode} workspaceName={workspaceName} root={root} summary={summary}
          changes={changes} selectedPath={selectedPath} />,
      },
    ]}
  />;
}

function CurrentInfo({ label, selectedPath, source, status, loading, error, parsed, workspaceName, mode }: {
  label: string | null;
  selectedPath: string | null;
  source: { type: "file"; file: WorkspaceFile } | { type: "git"; diff: WorkspaceDiff } | null;
  status: WorkspaceChanges["files"][number] | undefined;
  loading: boolean;
  error: string;
  parsed: ReturnType<typeof parsePatch> | null;
  workspaceName: string;
  mode: "files" | "git";
}) {
  const { t, locale } = useI18n();
  if (loading) return <p className="caption right-rail-note" role="status">{t("files.reading")}</p>;
  if (error && !source) return <p className="field-error" role="alert">{error}</p>;
  if (!selectedPath || !source) return <RightRailEmpty
    icon={mode === "files" ? <IconFile /> : <IconGit />}
    title={t("files.noFileOpen")}
    description={mode === "files" ? t("files.noFileOpenHint") : t("files.noChangeOpenHint")}
  />;
  return <>
    {error && <p className="caption right-rail-note" role="status">{t("files.refreshFailed", { error })}</p>}
    <section className="right-rail-section">
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        <FileGlyph path={selectedPath} />
        <span className="truncate right-rail-item-main mono" title={selectedPath}>{label}</span>
        {status && <span className={`git-status ${gitStatusMeta(status.status, locale).tone}`}>{gitStatusMeta(status.status, locale).code.trim() || "?"}</span>}
      </div>
      <div className="right-rail-kv"><span>{t("files.workspace")}</span><span>{workspaceName}</span></div>
      {status?.repository && <div className="right-rail-kv"><span>{t("files.gitRepository")}</span><span className="mono">{status.repository}</span></div>}
      <div className="right-rail-kv"><span>{t("files.path")}</span><span className="mono">{selectedPath}</span></div>
    </section>
    {source.type === "file" ? <>
      <section className="right-rail-section">
        <h3>{t("files.file")}</h3>
        <div className="right-rail-stats">
          <div className="right-rail-stat"><span>{t("files.size")}</span><strong>{formatSize(source.file.size)}</strong></div>
          <div className="right-rail-stat"><span>{t("files.type")}</span><strong>{source.file.kind}</strong></div>
          <div className="right-rail-stat"><span>{t("files.lineCountLabel")}</span><strong>{source.file.content === null ? "—" : source.file.content.split("\n").length.toLocaleString()}</strong></div>
          <div className="right-rail-stat"><span>{t("files.version")}</span><strong className="mono">{source.file.version ? source.file.version.slice(0, 8) : "—"}</strong></div>
        </div>
        {source.file.truncated && <p className="caption right-rail-note">{t("files.truncatedFileNote")}</p>}
        <p className="caption right-rail-note">{t("files.lineSelectionHint")}</p>
      </section>
    </> : <>
      <section className="right-rail-section">
        <h3>{t("files.diff")}</h3>
        <div className="right-rail-stats">
          <div className="right-rail-stat added"><span>{t("files.added")}</span><strong>+{parsed?.added ?? 0}</strong></div>
          <div className="right-rail-stat removed"><span>{t("files.removed")}</span><strong>−{parsed?.removed ?? 0}</strong></div>
          <div className="right-rail-stat"><span>Hunk</span><strong>{parsed?.hunks ?? 0}</strong></div>
          <div className="right-rail-stat"><span>{t("files.statusLabel")}</span><strong>{status ? gitStatusMeta(status.status, locale).label : "—"}</strong></div>
        </div>
        <div className="right-rail-kv"><span>{t("files.diffBase")}</span><span className="mono">{source.diff.base}</span></div>
        {source.diff.truncated && <p className="caption right-rail-note">{t("files.truncatedDiffNote")}</p>}
        <p className="caption right-rail-note">{t("files.mergedDiffNote")}</p>
      </section>
    </>}
  </>;
}

function WorkspaceInfo({ mode, workspaceName, root, summary, changes, selectedPath }: {
  mode: "files" | "git";
  workspaceName: string;
  root: string;
  summary: ReturnType<typeof summarizeChanges>;
  changes: WorkspaceChanges | null;
  selectedPath: string | null;
}) {
  const { t } = useI18n();
  return <>
    <section className="right-rail-section">
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        <IconFolder />
        <span className="truncate right-rail-item-main">{workspaceName}</span>
      </div>
      <div className="right-rail-kv"><span>{t("files.root")}</span><span className="mono">{root}</span></div>
      <div className="right-rail-kv"><span>{t("files.view")}</span><span>{mode === "files" ? "File" : "Git"}</span></div>
      {mode === "git" && changes?.repositories && changes.repositories.length > 0 && (
        <div className="right-rail-kv"><span>{t("files.gitRepository")}</span><span>{t("files.repositoryCount", { count: changes.repositories.length })}</span></div>
      )}
      <div className="right-rail-kv"><span>{t("files.currentFile")}</span><span className="mono">{selectedPath ?? "—"}</span></div>
    </section>
    {mode === "git" && <section className="right-rail-section">
      <h3>{t("files.workingTree")}</h3>
      {!changes?.isRepo ? <p className="caption right-rail-note">{t("files.notAGitRepo")}</p> : <>
        <div className="right-rail-stats">
          <div className="right-rail-stat"><span>{t("files.staged")}</span><strong>{summary.staged}</strong></div>
          <div className="right-rail-stat"><span>{t("files.unstaged")}</span><strong>{summary.unstaged}</strong></div>
          <div className="right-rail-stat"><span>{t("files.untracked")}</span><strong>{summary.untracked}</strong></div>
          <div className="right-rail-stat"><span>{t("files.total")}</span><strong>{summary.total}</strong></div>
        </div>
        <p className="caption right-rail-note">{t("files.gitReadOnlyNote")}</p>
      </>}
    </section>}
    {mode === "files" && <section className="right-rail-section">
      <p className="caption right-rail-note">{t("files.filesReloadNote")}</p>
    </section>}
  </>;
}
