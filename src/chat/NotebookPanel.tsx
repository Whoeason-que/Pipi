import type { AgentDefinition } from "../types";
import { Markdown } from "../Markdown";
import { useI18n } from "../i18n";
import { InspectorTabIcon } from "./inspector-tabs";
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
      {notebook.error ? (
        <p className="file-error">{notebook.error}</p>
      ) : !notebook.content ? (
        notebook.loading ? (
          <div className="inspector-loading" role="status">
            <span className="file-state-spinner" />
            {t("common.loading")}
          </div>
        ) : (
          <div className="inspector-empty">
            <span className="icon-tile inspector-empty-icon">
              <InspectorTabIcon tab="notebook" />
            </span>
            <strong>{t("inspector.notebookEmptyTitle")}</strong>
            <p>{t("inspector.notebookEmpty")}</p>
          </div>
        )
      ) : (
        <>
          {progress.total > 0 && (
            <div className="notebook-progress-row">
              <span className="notebook-progress-label">
                {progress.done} / {progress.total}
              </span>
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
            </div>
          )}
          <div className="notebook-body">
            <Markdown text={notebook.content} imageAgent={agent} />
          </div>
        </>
      )}
    </div>
  );
}
