import { IconAlertTriangle, IconCheck } from "../icons";
import { useI18n } from "../i18n";
import {
  backgroundResultLabel,
  type BackgroundResultView,
} from "./background-result";

/** 右栏时间线里的后台任务完成通知（不是用户气泡）。 */
export default function BackgroundResultCard({
  result,
}: {
  result: BackgroundResultView;
}) {
  const { t } = useI18n();
  const failed = result.status === "failed";
  return (
    <div
      className={`background-card ${failed ? "background-failed" : "background-done"}`}
      role="status"
    >
      <div className="background-heading">
        <span className="background-icon" aria-hidden="true">
          {failed ? <IconAlertTriangle size={16} /> : <IconCheck />}
        </span>
        <span className="background-title">
          {failed ? t("chat.backgroundFailed") : t("chat.backgroundDone")}
        </span>
        <span className="background-meta mono" title={result.jobId}>
          {backgroundResultLabel(result)}
          {result.exitCode != null ? ` · exit ${result.exitCode}` : ""}
        </span>
      </div>
      {result.error && <div className="background-error">{result.error}</div>}
      {result.output && (
        <details className="background-output">
          <summary>{t("inspector.output")}</summary>
          <pre>{result.output}</pre>
        </details>
      )}
    </div>
  );
}
