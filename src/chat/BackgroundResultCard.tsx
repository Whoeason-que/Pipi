import { IconAlertTriangle, IconCheck } from "../icons";
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
          {failed ? "后台任务失败" : "后台任务完成"}
        </span>
        <span className="background-meta mono" title={result.jobId}>
          {backgroundResultLabel(result)}
          {result.exitCode != null ? ` · exit ${result.exitCode}` : ""}
        </span>
      </div>
      {result.error && <div className="background-error">{result.error}</div>}
      {result.output && (
        <details className="background-output">
          <summary>输出</summary>
          <pre>{result.output}</pre>
        </details>
      )}
    </div>
  );
}
