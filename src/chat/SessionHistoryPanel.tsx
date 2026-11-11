import { useCallback, useEffect, useRef, useState } from "react";
import type { AgentDefinition, SessionCommitView } from "../types";
import { formatRuntimeError } from "../chat-runtime";
import { useI18n } from "../i18n";
import { invoke } from "../platform";
import { IconGitBranch, IconRefresh } from "../icons";
import { formatClock } from "./ToolDetails";

/** 同一天只显示时分秒；跨天补上「月/日」，避免旧提交只剩一个时间。 */
function formatCommitTime(timestamp: number): string {
  const date = new Date(timestamp);
  const time = formatClock(timestamp);
  return date.toDateString() === new Date().toDateString()
    ? time
    : `${date.getMonth() + 1}/${date.getDate()} ${time.slice(0, 5)}`;
}

interface Props {
  agent: AgentDefinition;
  sessionId: string | null;
  /** 运行中低频兜底：一轮结束（running true→false）后重新拉取提交。 */
  running: boolean;
  /** 只有切到这个 tab 时才拉取。 */
  active: boolean;
}

/** 右侧栏的会话 git tree：一轮一个 commit，展开看本轮新增的条目。 */
export default function SessionHistoryPanel({ agent, sessionId, running, active }: Props) {
  const { t } = useI18n();
  const [commits, setCommits] = useState<SessionCommitView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const request = useRef(0);
  const wasRunning = useRef(running);

  const kindLabels: Record<string, string> = {
    message: t("inspector.historyKindMessage"),
    compaction: t("inspector.historyKindCompaction"),
    context_edit: t("inspector.historyKindContextEdit"),
    model_change: t("inspector.historyKindModelChange"),
    env: t("inspector.historyKindEnv"),
    background_result: t("inspector.historyKindBackground"),
    subagent_run: t("inspector.historyKindSubagent"),
    custom: t("inspector.historyKindCustom"),
  };

  const refresh = useCallback(async () => {
    if (!sessionId) {
      request.current += 1;
      setCommits([]);
      setLoading(false);
      setError(null);
      return;
    }
    const id = ++request.current;
    setLoading(true);
    try {
      const history = await invoke("session_history", {
        agentName: agent.name,
        sessionId,
      });
      if (id !== request.current) return;
      setCommits(Array.isArray(history) ? history : []);
      setError(null);
    } catch (errorValue) {
      if (id === request.current) setError(formatRuntimeError(errorValue));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, [agent.name, sessionId]);

  // 切换会话时先清掉上一个会话的树，避免加载期间串台。
  useEffect(() => {
    setCommits([]);
    setExpanded(new Set());
    setError(null);
  }, [sessionId]);

  // 打开 tab 时拉取；一轮从运行中落到空闲时再拉一次（提交点在轮末）。
  useEffect(() => {
    if (active) void refresh();
  }, [active, refresh]);
  useEffect(() => {
    if (!active) {
      wasRunning.current = running;
      return;
    }
    if (wasRunning.current && !running) void refresh();
    wasRunning.current = running;
  }, [active, running, refresh]);

  const toggle = (id: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <div className="session-history">
      <section className="ip">
        <h4>
          <IconGitBranch />
          {t("inspector.historyTitle")}
          {commits.length > 0 && <span className="history-count">{commits.length}</span>}
          <button
            type="button"
            className="history-refresh"
            onClick={() => void refresh()}
            disabled={loading || !sessionId}
            title={t("inspector.historyRefresh")}
            aria-label={t("inspector.historyRefresh")}
          >
            <IconRefresh />
          </button>
        </h4>
        {error && (
          <p className="file-error" role="status">
            {t("inspector.historyLoadFailed", { error })}
          </p>
        )}
        {!sessionId ? (
          <div className="inspector-empty">
            <span className="icon-tile inspector-empty-icon">
              <IconGitBranch size={19} />
            </span>
            <strong>{t("inspector.historyEmptyTitle")}</strong>
            <p>{t("inspector.historyNoSession")}</p>
          </div>
        ) : commits.length === 0 ? (
          loading ? (
            <div className="inspector-loading" role="status">
              <span className="file-state-spinner" />
              {t("common.loading")}
            </div>
          ) : (
            <div className="inspector-empty">
              <span className="icon-tile inspector-empty-icon">
                <IconGitBranch size={19} />
              </span>
              <strong>{t("inspector.historyEmptyTitle")}</strong>
              <p>{t("inspector.historyEmptyHint")}</p>
            </div>
          )
        ) : (
          <ol className="history-list">
            {commits.map((commit) => {
              const open = expanded.has(commit.id);
              const title = commit.summary.replace(/^user:\s*/, "");
              return (
                <li
                  key={commit.id}
                  className={`history-item${commit.isTip ? " tip" : ""}`}
                >
                  <span className="history-dot" aria-hidden="true" />
                  <button
                    type="button"
                    className="history-head"
                    aria-expanded={open}
                    onClick={() => toggle(commit.id)}
                  >
                    <span className="history-main">
                      <span className="history-title" title={title}>
                        {title || t("inspector.historyEmptySummary")}
                      </span>
                      <span className="history-meta">
                        {commit.turn != null && (
                          <span>{t("inspector.historyTurn", { turn: commit.turn })}</span>
                        )}
                        <span>{formatCommitTime(commit.timestamp)}</span>
                        <span className="history-hash">{commit.id.slice(0, 7)}</span>
                        {commit.isTip && (
                          <span className="history-tip">{t("inspector.historyTip")}</span>
                        )}
                      </span>
                    </span>
                    <span className="history-entry-count">{commit.entries.length}</span>
                  </button>
                  {open && (
                    <ul className="history-entries">
                      {commit.entries.map((entry) => (
                        <li
                          key={`${commit.id}-${entry.seq}-${entry.id}`}
                          className="history-entry"
                        >
                          <span
                            className={`history-kind history-kind-${entry.kind}`}
                          >
                            {kindLabels[entry.kind] ?? entry.kind}
                          </span>
                          <span className="history-seq">#{entry.seq}</span>
                          <span className="history-entry-id">{entry.id}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              );
            })}
          </ol>
        )}
      </section>
    </div>
  );
}
