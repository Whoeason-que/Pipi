// 工作区右栏：当前文件/Git 改动的元数据与工作区摘要。
// 预览与 diff 仍在主区，右栏不重复正文。

import { useMemo } from "react";
import { useRightRail, RightRail } from "../app/right-rail";
import { useCopyFeedback } from "../components/use-copy";
import { IconCopy, IconFile, IconGit, IconFolder } from "../icons";
import type { WorkspaceChanges, WorkspaceDiff, WorkspaceFile } from "./types";
import { FileGlyph } from "./FileGlyph";
import { formatSize } from "./FilePreview";
import { gitStatusMeta, parsePatch } from "./diff";

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
  const parsed = useMemo(() => source?.type === "git" ? parsePatch(source.diff.patch) : null, [source]);
  const status = selectedPath ? changes?.files.find((file) => file.path === selectedPath) : undefined;
  const summary = summarize(changes?.files ?? []);
  const title = mode === "files" ? "文件信息" : "Git 改动";
  const label = source?.type === "file" ? source.file.path.split("/").pop() ?? source.file.path
    : source?.type === "git" ? source.diff.path.split("/").pop() ?? source.diff.path
      : null;

  return <RightRail
    controller={controller}
    id="workspace-rail"
    label={title}
    subtitle={workspaceName}
    icon={mode === "files" ? <IconFile /> : <IconGit />}
    actions={selectedPath ? <button type="button" className="icon-btn" title={clipboard.label === "复制" ? "复制路径" : clipboard.label}
      aria-label="复制文件路径" onClick={() => void clipboard.copy(selectedPath)}><IconCopy /></button> : undefined}
    tabs={[
      {
        id: "current",
        label: "当前",
        content: <CurrentInfo label={label} selectedPath={selectedPath} source={source} status={status}
          loading={loading} error={error} parsed={parsed} workspaceName={workspaceName} mode={mode} />,
      },
      {
        id: "workspace",
        label: "工作区",
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
  if (loading) return <p className="right-rail-note" role="status">正在读取…</p>;
  if (error && !source) return <p className="field-error" role="alert">{error}</p>;
  if (!selectedPath || !source) return <div className="right-rail-empty">
    {mode === "files" ? <IconFile /> : <IconGit />}
    <strong>还没有打开文件</strong>
    <p>{mode === "files" ? "在左侧文件树中选择文件，这里会显示类型、大小和版本。" : "在左侧改动列表中选择文件，这里会显示状态、比较基准和增删行数。"}</p>
  </div>;
  return <>
    {error && <p className="right-rail-note" role="status">刷新失败，显示上次信息：{error}</p>}
    <section className="right-rail-section">
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        <FileGlyph path={selectedPath} />
        <span className="right-rail-item-main mono" title={selectedPath}>{label}</span>
        {status && <span className={`git-status ${gitStatusMeta(status.status).tone}`}>{gitStatusMeta(status.status).code.trim() || "?"}</span>}
      </div>
      <div className="right-rail-kv"><span>工作区</span><span>{workspaceName}</span></div>
      {status?.repository && <div className="right-rail-kv"><span>Git 仓库</span><span className="mono">{status.repository}</span></div>}
      <div className="right-rail-kv"><span>路径</span><span className="mono">{selectedPath}</span></div>
    </section>
    {source.type === "file" ? <>
      <section className="right-rail-section">
        <h3>文件</h3>
        <div className="right-rail-stats">
          <div className="right-rail-stat"><span>大小</span><strong>{formatSize(source.file.size)}</strong></div>
          <div className="right-rail-stat"><span>类型</span><strong>{source.file.kind}</strong></div>
          <div className="right-rail-stat"><span>行数</span><strong>{source.file.content === null ? "—" : source.file.content.split("\n").length.toLocaleString()}</strong></div>
          <div className="right-rail-stat"><span>版本</span><strong className="mono">{source.file.version ? source.file.version.slice(0, 8) : "—"}</strong></div>
        </div>
        {source.file.truncated && <p className="right-rail-note">内容已截断，只能引用文件路径，不能生成完整选区。</p>}
        <p className="right-rail-note">在主区预览点击行号选择范围，再使用底部的「引用到会话」。</p>
      </section>
    </> : <>
      <section className="right-rail-section">
        <h3>差异</h3>
        <div className="right-rail-stats">
          <div className="right-rail-stat added"><span>新增</span><strong>+{parsed?.added ?? 0}</strong></div>
          <div className="right-rail-stat removed"><span>删除</span><strong>−{parsed?.removed ?? 0}</strong></div>
          <div className="right-rail-stat"><span>Hunk</span><strong>{parsed?.hunks ?? 0}</strong></div>
          <div className="right-rail-stat"><span>状态</span><strong>{status ? gitStatusMeta(status.status).label : "—"}</strong></div>
        </div>
        <div className="right-rail-kv"><span>比较基准</span><span className="mono">{source.diff.base}</span></div>
        {source.diff.truncated && <p className="right-rail-note">差异内容已截断，请使用外部 Git 工具查看完整改动。</p>}
        <p className="right-rail-note">已暂存与未暂存差异合并展示；未跟踪文件没有 Git 基线。</p>
      </section>
    </>}
  </>;
}

function WorkspaceInfo({ mode, workspaceName, root, summary, changes, selectedPath }: {
  mode: "files" | "git";
  workspaceName: string;
  root: string;
  summary: ReturnType<typeof summarize>;
  changes: WorkspaceChanges | null;
  selectedPath: string | null;
}) {
  return <>
    <section className="right-rail-section">
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        <IconFolder />
        <span className="right-rail-item-main">{workspaceName}</span>
      </div>
      <div className="right-rail-kv"><span>根目录</span><span className="mono">{root}</span></div>
      <div className="right-rail-kv"><span>视图</span><span>{mode === "files" ? "File" : "Git"}</span></div>
      {mode === "git" && changes?.repositories && changes.repositories.length > 0 && (
        <div className="right-rail-kv"><span>Git 仓库</span><span>{changes.repositories.length} 个</span></div>
      )}
      <div className="right-rail-kv"><span>当前文件</span><span className="mono">{selectedPath ?? "—"}</span></div>
    </section>
    {mode === "git" && <section className="right-rail-section">
      <h3>工作树</h3>
      {!changes?.isRepo ? <p className="right-rail-note">当前工作区不属于 Git 仓库。</p> : <>
        <div className="right-rail-stats">
          <div className="right-rail-stat"><span>已暂存</span><strong>{summary.staged}</strong></div>
          <div className="right-rail-stat"><span>未暂存</span><strong>{summary.unstaged}</strong></div>
          <div className="right-rail-stat"><span>未跟踪</span><strong>{summary.untracked}</strong></div>
          <div className="right-rail-stat"><span>合计</span><strong>{summary.total}</strong></div>
        </div>
        <p className="right-rail-note">Git 视图只读：预览使用固定参数与输出上限，不执行仓库配置里的外部 diff/filter。</p>
      </>}
    </section>}
    {mode === "files" && <section className="right-rail-section">
      <p className="right-rail-note">右上角刷新会重新读取目录；文件内容、标签与滚动位置按 Agent 工作区隔离保存。</p>
    </section>}
  </>;
}

function summarize(files: WorkspaceChanges["files"]) {
  let staged = 0;
  let unstaged = 0;
  let untracked = 0;
  for (const file of files) {
    const meta = gitStatusMeta(file.status);
    if (meta.tone === "untracked") untracked += 1;
    else {
      if (meta.staged) staged += 1;
      if (meta.unstaged) unstaged += 1;
    }
  }
  return { staged, unstaged, untracked, total: files.length };
}
