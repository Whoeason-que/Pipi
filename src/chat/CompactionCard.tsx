import type { ChatEntry } from "../chat-runtime";
import { IconAlertTriangle, IconCheck, IconRefresh } from "../icons";
import { Markdown } from "../Markdown";
import type { ImageAgent } from "../files/MarkdownImage";

export default function CompactionCard({ entry, agent }: { entry: ChatEntry; agent: ImageAgent }) {
  const failed = entry.status === "error";
  const running = entry.transient && !failed && !entry.summary;
  const icon = <span className="compaction-icon" aria-hidden="true">{failed ? <IconAlertTriangle size={16} /> : running ? <IconRefresh /> : <IconCheck />}</span>;
  const title = entry.text.replace(/^♻\s*/, "");
  if (entry.summary) return <details className="compaction-card compaction-done">
    <summary>{icon}<span className="compaction-title">{title}</span><span className="compaction-chevron" aria-hidden="true">⌄</span></summary>
    <div className="compaction-summary"><Markdown text={entry.summary} imageAgent={agent} /></div>
  </details>;
  return <div className={`compaction-card ${failed ? "compaction-failed" : running ? "compaction-running" : "compaction-done"}`} role="status">
    <div className="compaction-heading">{icon}<span className="compaction-title">{title}</span></div>
    {entry.errorMessage && <div className="compaction-error">{entry.errorMessage}</div>}
  </div>;
}
