import type { AgentDefinition } from "../types";
import { Markdown } from "../Markdown";
import { useI18n } from "../i18n";
import { parsePlan } from "./notebook-plan";
import type { NotebookSnapshot } from "./notebook";

/** 右栏「笔记本」：Agent 维护的会话工作文档（计划清单 + 进度 + 走查）。 */
export function NotebookPanel({
  notebook,
  agent,
}: {
  notebook: NotebookSnapshot;
  agent: AgentDefinition;
}) {
  const { t } = useI18n();
  const progress = parsePlan(notebook.content);
  const percent = progress.total
    ? Math.round((progress.done / progress.total) * 100)
    : 0;

  return (
    <div className="file-workbench notebook-panel">
      <div className="file-workbench-header">
        <strong>{t("inspector.notebook")}</strong>
        {progress.total > 0 && (
          <small className="notebook-progress-label">
            {progress.done} / {progress.total}
          </small>
        )}
      </div>
      {notebook.error ? (
        <p className="file-notice">{notebook.error}</p>
      ) : !notebook.content ? (
        <p className="file-notice">
          {notebook.loading
            ? t("common.loading")
            : t("inspector.notebookEmpty")}
        </p>
      ) : (
        <>
          {progress.total > 0 && (
            <span
              className="notebook-progress"
              role="progressbar"
              aria-label={t("inspector.planProgress")}
              aria-valuenow={progress.done}
              aria-valuemin={0}
              aria-valuemax={progress.total}
            >
              <i style={{ width: `${percent}%` }} />
            </span>
          )}
          <div className="notebook-body">
            <Markdown text={notebook.content} imageAgent={agent} />
          </div>
        </>
      )}
    </div>
  );
}
