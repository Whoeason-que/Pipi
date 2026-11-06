import AgentAvatar from "../components/AgentAvatar";
import { useI18n } from "../i18n";
import {
  IconArrowUpRight,
  IconClock,
  IconFolder,
  IconGear,
  IconMessage,
  IconPlus,
} from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { sessionKey, sessionKeyPrefix } from "../session-key.ts";
import type { AgentSection } from "./navigation";

interface WorkspaceOverviewProps {
  section: AgentSection;
  agents: AgentDefinition[];
  agent?: AgentDefinition;
  sessions: Record<string, SessionSummaryView[]>;
  running: Set<string>;
  onAgent: (name: string) => void;
  onSession: (name: string, id: string) => void;
  onNewSession: (name: string) => void;
  onConfigure: (name: string) => void;
  onCreate: () => void;
}

type Activity = { agent: AgentDefinition; session: SessionSummaryView };

function relativeTime(
  timestamp: number,
  now: number,
  format: Intl.RelativeTimeFormat,
): string {
  const diff = timestamp - now;
  const distance = Math.abs(diff);
  if (distance < 60_000) return format.format(0, "second");
  if (distance < 3_600_000) return format.format(Math.trunc(diff / 60_000), "minute");
  if (distance < 86_400_000) return format.format(Math.trunc(diff / 3_600_000), "hour");
  if (distance < 7 * 86_400_000) return format.format(Math.trunc(diff / 86_400_000), "day");
  return "";
}

function SessionActivity({
  items,
  running,
  onSession,
}: {
  items: Activity[];
  running: Set<string>;
  onSession: WorkspaceOverviewProps["onSession"];
}) {
  const { locale, t } = useI18n();
  const date = new Intl.DateTimeFormat(locale === "zh" ? "zh-CN" : "en", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
  const relative = new Intl.RelativeTimeFormat(
    locale === "zh" ? "zh-CN" : "en",
    { numeric: "auto" },
  );
  const now = Date.now();
  return (
    <div className="activity-list">
      {items.map(({ agent, session }) => {
        const isRunning = running.has(sessionKey(agent.name, session.id));
        const lastActive = session.lastActive;
        const absolute = lastActive ? date.format(lastActive) : "";
        const timeLabel = lastActive
          ? relativeTime(lastActive, now, relative) || absolute
          : t("workspace.noActivity");
        return (
          <button
            type="button"
            key={sessionKey(agent.name, session.id)}
            className={`activity-row${isRunning ? " is-running" : ""}`}
            onClick={() => onSession(agent.name, session.id)}
          >
            <span className="activity-name">
              <b>
                <span className="activity-title">
                  {session.title || t("workspace.untitled")}
                </span>
                {isRunning && (
                  <em className="activity-badge">
                    {t("workspace.runningBadge")}
                  </em>
                )}
              </b>
              <small>
                <span>{agent.name}</span>
                <span>
                  {t(
                    session.messageCount === 1
                      ? "workspace.messagesOne"
                      : "workspace.messages",
                    { count: session.messageCount },
                  )}
                </span>
              </small>
            </span>
            <time
              dateTime={
                lastActive ? new Date(lastActive).toISOString() : undefined
              }
              title={absolute || undefined}
            >
              {timeLabel}
            </time>
            <span className="activity-arrow">
              <IconArrowUpRight />
            </span>
          </button>
        );
      })}
    </div>
  );
}

export default function WorkspaceOverview({
  section,
  agents,
  agent,
  sessions,
  running,
  onAgent,
  onSession,
  onNewSession,
  onConfigure,
  onCreate,
}: WorkspaceOverviewProps) {
  const { t } = useI18n();
  const subagents = section === "subagents";
  const visible = agent ? [agent] : agents;
  const recent = visible
    .flatMap((item) =>
      (sessions[item.name] ?? []).map((session) => ({ agent: item, session })),
    )
    .sort((a, b) => b.session.lastActive - a.session.lastActive);
  const isRunning = (item: Activity) =>
    running.has(sessionKey(item.agent.name, item.session.id));
  // 正在运行的会话置顶，剩下的按最近活动排序。
  const active = recent.filter(isRunning);
  const activity = recent.filter((item) => !isRunning(item)).slice(0, 8);

  return (
    <div className="page-scroll">
      <header
        className={`page-heading page-heading-spaced${agent ? " agent-overview-heading" : " overview-workspace-heading"}`}
      >
        {agent && (
          <AgentAvatar
            name={agent.name}
            subagent={agent.subagent}
            className="overview-avatar"
          />
        )}
        <div className="page-heading-intro">
          <h1>
            {agent?.name ??
              t(
                subagents
                  ? "workspace.subagentsTitle"
                  : agents.length
                    ? "workspace.title"
                    : "workspace.firstTitle",
              )}
          </h1>
          <p>
            {agent?.description ||
              t(
                agent
                  ? "workspace.agentDescription"
                  : subagents
                    ? "workspace.subagentsDescription"
                    : "workspace.description",
              )}
          </p>
        </div>
        {(agent || !subagents) && (
          <div className="page-heading-actions">
            {agent ? (
              <>
                <button
                  type="button"
                  className="btn primary"
                  onClick={() => onNewSession(agent.name)}
                >
                  <IconPlus />
                  {t("nav.newSession")}
                </button>
                {recent[0] && (
                  <button
                    type="button"
                    className="btn"
                    onClick={() => onSession(agent.name, recent[0].session.id)}
                  >
                    <IconClock />
                    {t("workspace.continue")}
                  </button>
                )}
                <button
                  type="button"
                  className="btn ghost"
                  onClick={() => onConfigure(agent.name)}
                >
                  <IconGear />
                  {t("workspace.configure")}
                </button>
              </>
            ) : (
              <button type="button" className="btn primary" onClick={onCreate}>
                <IconPlus />
                {t("nav.newAgent")}
              </button>
            )}
          </div>
        )}
      </header>

      {active.length > 0 && (
        <section className="page-section overview-active">
          <div className="section-heading section-heading-ruled">
            <h2>{t("workspace.activeWork")} <span className="section-count">{active.length}</span></h2>
            <span>{t("workspace.activeHint")}</span>
          </div>
          <SessionActivity items={active} running={running} onSession={onSession} />
        </section>
      )}

      {agent && (
        <section
          className="surface agent-facts"
          aria-label={t("workspace.agentInfo")}
        >
          <div>
            <span>{t("workspace.directory")}</span>
            <strong title={agent.workspace ?? undefined}>
              {agent.workspace || t("workspace.defaultDirectory")}
            </strong>
          </div>
          <div>
            <span>{t("workspace.defaultModel")}</span>
            <strong>
              {agent.provider?.id || agent.model || t("workspace.noModel")}
            </strong>
          </div>
          <div>
            <span>{t("workspace.permissions")}</span>
            <strong>
              {t(
                agent.permissions.tools.length === 1
                  ? "workspace.toolsOne"
                  : "workspace.tools",
                { count: agent.permissions.tools.length },
              )}{" "}
              / {t(`workspace.sandbox.${agent.permissions.sandbox}`)}
            </strong>
          </div>
        </section>
      )}

      {!agent && (
        <section className="page-section">
          <div className="section-heading section-heading-ruled">
            <h2>
              {t(subagents ? "nav.subagents" : "workspace.yourAgents")}
              <span className="section-count">{agents.length}</span>
            </h2>
            <span>
              {t(
                subagents ? "workspace.subagentsHint" : "workspace.agentsHint",
              )}
            </span>
          </div>
          <div className="agent-cards">
            {agents.map((item) => {
              const itemRunning =
                [...running].filter((key) =>
                  key.startsWith(sessionKeyPrefix(item.name)),
                ).length > 0;
              const sessionCount = (sessions[item.name] ?? []).length;
              return (
                <button
                  type="button"
                  className="surface surface-interactive agent-card"
                  key={item.name}
                  onClick={() => onAgent(item.name)}
                >
                  <div className="agent-card-top">
                    <AgentAvatar name={item.name} subagent={item.subagent} />
                    <div className="agent-card-identity">
                      <b>{item.name}</b>
                      <span className="agent-card-sub">
                        <span>{item.subagent ? "Subagent" : "Agent"}</span>
                        <span>
                          {t(
                            sessionCount === 1
                              ? "common.sessionCountOne"
                              : "common.sessionCount",
                            { count: sessionCount },
                          )}
                        </span>
                        {itemRunning && (
                          <span className="agent-card-live">
                            <i />
                            {t("workspace.runningBadge")}
                          </span>
                        )}
                      </span>
                    </div>
                    <span className="activity-arrow">
                      <IconArrowUpRight />
                    </span>
                  </div>
                  <p className="agent-card-copy">
                    {item.description || t("workspace.agentPlaceholder")}
                  </p>
                  <div className="agent-card-footer">
                    <span
                      className="agent-card-directory"
                      title={item.workspace || t("workspace.defaultDirectory")}
                    >
                      <IconFolder />
                      <span>
                        {item.workspace || t("workspace.defaultDirectory")}
                      </span>
                    </span>
                    <span
                      className="agent-card-model"
                      title={item.provider?.id || item.model || undefined}
                    >
                      <span>{t("workspace.defaultModel")}</span>
                      <strong>
                        {item.provider?.id ||
                          item.model ||
                          t("workspace.noModel")}
                      </strong>
                    </span>
                  </div>
                </button>
              );
            })}
          </div>
          {!agents.length && (
            <div className="empty-card">
              <span className="empty-icon">
                <IconPlus />
              </span>
              <h3>
                {t(
                  subagents
                    ? "workspace.subagentsEmptyTitle"
                    : "workspace.createFirst",
                )}
              </h3>
              <p>
                {t(
                  subagents
                    ? "workspace.subagentsEmpty"
                    : "workspace.createHint",
                )}
              </p>
              {!subagents && (
                <button type="button" className="btn primary" onClick={onCreate}>
                  {t("nav.newAgent")}
                </button>
              )}
            </div>
          )}
        </section>
      )}

      <section className="page-section">
        <div className="section-heading section-heading-ruled">
          <h2>{t("workspace.recent")}</h2>
          <span>
            {t(
              agent
                ? "workspace.resumeHint"
                : subagents
                  ? "workspace.allSubagents"
                  : "workspace.allAgents",
            )}
          </span>
        </div>
        {activity.length ? (
          <SessionActivity
            items={activity}
            running={running}
            onSession={onSession}
          />
        ) : !active.length ? (
          <div className="empty-card compact">
            <IconMessage />
            <p>
              {t(agent ? "workspace.emptyAgent" : "workspace.emptyWorkspace")}
            </p>
          </div>
        ) : <p className="caption overview-no-recent">{t("workspace.noOtherActivity")}</p>}
      </section>
      <footer className="caption overview-footer">
        <span>
          {t("workspace.shortcutHint")} <kbd>Ctrl / ⌘ K</kbd>
        </span>
        <span>{t("workspace.footer")}</span>
      </footer>
    </div>
  );
}
