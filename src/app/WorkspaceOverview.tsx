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
const sessionKey = (agent: string, session: string) =>
  `${agent}\u0000${session}`;

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
  return (
    <div className="activity-list">
      {items.map(({ agent, session }) => {
        const active = running.has(sessionKey(agent.name, session.id));
        return (
          <button
            type="button"
            key={sessionKey(agent.name, session.id)}
            className="activity-row"
            onClick={() => onSession(agent.name, session.id)}
          >
            <span className={`activity-icon${active ? " running" : ""}`}>
              <IconMessage />
            </span>
            <span className="activity-name">
              <b>{session.title || t("workspace.untitled")}</b>
              <small>
                <span>{agent.name}</span>
                <span>
                  {t("workspace.messages", { count: session.messageCount })}
                </span>
              </small>
            </span>
            {active && (
              <span className="activity-status">
                <i className="activity-dot running" />
                {t("common.running")}
              </span>
            )}
            <time
              dateTime={
                session.lastActive
                  ? new Date(session.lastActive).toISOString()
                  : undefined
              }
            >
              {session.lastActive
                ? date.format(session.lastActive)
                : t("workspace.noActivity")}
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
  const runningCount = [...running].filter((key) =>
    visible.some((item) => key.startsWith(`${item.name}\u0000`)),
  ).length;
  const currentRunning = recent.filter((item) =>
    running.has(sessionKey(item.agent.name, item.session.id)),
  );

  return (
    <div className="overview-page">
      <header
        className={`overview-heading${agent ? " agent-overview-heading" : ""}`}
      >
        {agent && (
          <AgentAvatar
            name={agent.name}
            subagent={agent.subagent}
            className="overview-avatar"
          />
        )}
        <div className="overview-intro">
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
          <div className="overview-actions">
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
              !subagents && (
                <button
                  type="button"
                  className="btn primary"
                  onClick={onCreate}
                >
                  <IconPlus />
                  {t("nav.newAgent")}
                </button>
              )
            )}
          </div>
        </div>
      </header>

      <div className="overview-summary">
        <span className={runningCount ? "summary-running" : ""}>
          <i className={`activity-dot${runningCount ? " running" : ""}`} />
          {runningCount
            ? t("workspace.running", { count: runningCount })
            : t("workspace.ready")}
        </span>
        {!agent && (
          <span>
            <b>{agents.length}</b>
            {t(subagents ? "nav.subagents" : "nav.agents")}
          </span>
        )}
        <span>
          <b>{recent.length}</b>
          {t("workspace.sessions")}
        </span>
        <span className="summary-local">
          <IconFolder />
          {t("workspace.localFiles")}
        </span>
      </div>

      {agent && (
        <section className="agent-facts" aria-label={t("workspace.agentInfo")}>
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
              {t("workspace.tools", { count: agent.permissions.tools.length })}{" "}
              / {t(`workspace.sandbox.${agent.permissions.sandbox}`)}
            </strong>
          </div>
        </section>
      )}

      {!agent && (
        <section className="overview-section">
          <div className="section-heading">
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
              const active = [...running].some((key) =>
                key.startsWith(`${item.name}\u0000`),
              );
              return (
                <button
                  type="button"
                  className="agent-card"
                  key={item.name}
                  onClick={() => onAgent(item.name)}
                >
                  <div className="agent-card-top">
                    <AgentAvatar name={item.name} subagent={item.subagent} />
                    <span
                      className={`agent-card-state${active ? " running" : ""}`}
                    >
                      <i
                        className={`activity-dot${active ? " running" : ""}`}
                      />
                      {t(active ? "common.running" : "workspace.ready")}
                    </span>
                    <span className="activity-arrow">
                      <IconArrowUpRight />
                    </span>
                  </div>
                  <div className="agent-card-copy">
                    <b>{item.name}</b>
                    <p>{item.description || t("workspace.agentPlaceholder")}</p>
                  </div>
                  <div className="agent-card-footer">
                    <span className="agent-kind">
                      {item.subagent ? "Subagent" : "Agent"}
                    </span>
                    <span>
                      <IconMessage />
                      {t("workspace.sessionCount", {
                        count: (sessions[item.name] ?? []).length,
                      })}
                    </span>
                  </div>
                </button>
              );
            })}
          </div>
          {!agents.length && (
            <div className="overview-empty">
              <span className="empty-agent-icon">
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
                <button
                  type="button"
                  className="btn primary"
                  onClick={onCreate}
                >
                  {t("nav.newAgent")}
                </button>
              )}
            </div>
          )}
        </section>
      )}

      {currentRunning.length > 0 && (
        <section className="overview-section">
          <div className="section-heading">
            <h2>{t("workspace.inProgress")}</h2>
            <span>{t("workspace.backgroundHint")}</span>
          </div>
          <SessionActivity
            items={currentRunning}
            running={running}
            onSession={onSession}
          />
        </section>
      )}
      <section className="overview-section">
        <div className="section-heading">
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
        {recent.length ? (
          <SessionActivity
            items={recent.slice(0, 8)}
            running={running}
            onSession={onSession}
          />
        ) : (
          <div className="overview-empty compact">
            <IconMessage />
            <p>
              {t(agent ? "workspace.emptyAgent" : "workspace.emptyWorkspace")}
            </p>
          </div>
        )}
      </section>
      <footer className="overview-footer">
        <span>
          {t("workspace.shortcutHint")} <kbd>Ctrl / ⌘ K</kbd>
        </span>
        <span>{t("workspace.footer")}</span>
      </footer>
    </div>
  );
}
