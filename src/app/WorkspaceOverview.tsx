import { useState } from "react";
import AgentAvatar from "../components/AgentAvatar";
import ActionMenu from "../components/ActionMenu";
import { useI18n } from "../i18n";
import {
  IconArrowUpRight,
  IconChevronRight,
  IconClock,
  IconFile,
  IconFolder,
  IconGear,
  IconGit,
  IconMessage,
  IconPlus,
} from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";
import { sessionKey, sessionKeyPrefix } from "../session-key.ts";
import type { AgentSection } from "./navigation";
import type { WorkspaceGroupInfo } from "./workspaces";

interface WorkspaceOverviewProps {
  section: AgentSection;
  agents: AgentDefinition[];
  agent?: AgentDefinition;
  sessions: Record<string, SessionSummaryView[]>;
  running: Set<string>;
  /** 主 Agent 按工作区分组；subagents 分区或单 Agent 视图传 null（维持平铺）。 */
  groups: WorkspaceGroupInfo[] | null;
  groupActions: {
    onOpenFiles: (path: string) => void;
    onOpenGit: (path: string) => void;
    onRemoveCustom: (path: string) => void;
  };
  onAgent: (name: string) => void;
  onSession: (name: string, id: string) => void;
  onNewSession: (name: string) => void;
  onConfigure: (name: string) => void;
  onCreate: () => void;
}

type Activity = { agent: AgentDefinition; session: SessionSummaryView };

/** 折叠体 id：path 转义成 id 安全字符，encodeURIComponent 保证不同 path 不撞车。 */
function groupBodyId(path: string): string {
  return `workspace-group-${encodeURIComponent(path).replace(/[^a-zA-Z0-9_-]/g, "-")}`;
}

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

/** Agent 卡片：平铺网格与工作区分组体共用同一份标记。 */
function AgentCard({
  item,
  sessions,
  running,
  onAgent,
}: {
  item: AgentDefinition;
  sessions: Record<string, SessionSummaryView[]>;
  running: Set<string>;
  onAgent: WorkspaceOverviewProps["onAgent"];
}) {
  const { t } = useI18n();
  const itemRunning =
    [...running].filter((key) =>
      key.startsWith(sessionKeyPrefix(item.name)),
    ).length > 0;
  const sessionCount = (sessions[item.name] ?? []).length;
  return (
    <button
      type="button"
      className="surface surface-interactive agent-card"
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
          <span>{item.workspace || t("workspace.defaultDirectory")}</span>
        </span>
        <span
          className="agent-card-model"
          title={item.provider?.id || item.model || undefined}
        >
          <span>{t("workspace.defaultModel")}</span>
          <strong>
            {item.provider?.id || item.model || t("workspace.noModel")}
          </strong>
        </span>
      </div>
    </button>
  );
}

/** 零 Agent 的首创引导：平铺模式与分组模式的空仓兜底共用。 */
function EmptyAgentsCard({
  subagents,
  onCreate,
}: {
  subagents: boolean;
  onCreate: WorkspaceOverviewProps["onCreate"];
}) {
  const { t } = useI18n();
  return (
    <div className="empty-card">
      <span className="empty-icon">
        <IconPlus />
      </span>
      <h3>
        {t(
          subagents ? "workspace.subagentsEmptyTitle" : "workspace.createFirst",
        )}
      </h3>
      <p>
        {t(subagents ? "workspace.subagentsEmpty" : "workspace.createHint")}
      </p>
      {!subagents && (
        <button type="button" className="btn primary" onClick={onCreate}>
          {t("nav.newAgent")}
        </button>
      )}
    </div>
  );
}

/** 工作区分组区块：组头（折叠 + 元信息 + 打开入口）+ 组体（Agent 卡片网格）。 */
function AgentGroup({
  group,
  collapsed,
  onToggle,
  sessions,
  running,
  onAgent,
  actions,
}: {
  group: WorkspaceGroupInfo;
  collapsed: boolean;
  onToggle: (path: string) => void;
  sessions: Record<string, SessionSummaryView[]>;
  running: Set<string>;
  onAgent: WorkspaceOverviewProps["onAgent"];
  actions: WorkspaceOverviewProps["groupActions"];
}) {
  const { t } = useI18n();
  const bodyId = groupBodyId(group.path);
  // 组内会话数合计：各组自算，与卡片上的单 Agent 口径（sessions[name].length）一致。
  const sessionTotal = group.agents.reduce(
    (sum, item) => sum + (sessions[item.name]?.length ?? 0),
    0,
  );
  const removable = group.custom && group.agents.length === 0;
  return (
    <article className="surface workspace-group">
      <div className="workspace-group-header">
        <button
          type="button"
          className="workspace-group-toggle"
          aria-expanded={!collapsed}
          aria-controls={bodyId}
          onClick={() => onToggle(group.path)}
        >
          <IconChevronRight expanded={!collapsed} />
          <span className="workspace-group-icon">
            <IconFolder />
          </span>
          <span
            className="workspace-group-name"
            title={group.path || undefined}
          >
            {/* 兜底组（path 为空）= 未设置工作区的 Agent，展示名走翻译。 */}
            {group.path ? group.name : t("workspace.groupUngrouped")}
          </span>
          <span className="workspace-group-meta">
            <span>
              {t(
                group.agents.length === 1
                  ? "workspace.groupAgentCountOne"
                  : "workspace.groupAgentCount",
                { count: group.agents.length },
              )}
            </span>
            <span>
              {t(
                sessionTotal === 1
                  ? "workspace.groupSessionCountOne"
                  : "workspace.groupSessionCount",
                { count: sessionTotal },
              )}
            </span>
          </span>
        </button>
        <div className="workspace-group-actions">
          <button
            type="button"
            className="icon-btn"
            aria-label={t("workspace.groupOpenFiles")}
            title={t("workspace.groupOpenFiles")}
            onClick={() => actions.onOpenFiles(group.path)}
          >
            <IconFile />
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label={t("workspace.groupOpenGit")}
            title={t("workspace.groupOpenGit")}
            onClick={() => actions.onOpenGit(group.path)}
          >
            <IconGit />
          </button>
          {removable && (
            <ActionMenu
              label={t("workspace.groupRemoveCustom")}
              actions={[
                {
                  label: t("workspace.groupRemoveCustom"),
                  run: () => actions.onRemoveCustom(group.path),
                  danger: true,
                },
              ]}
            />
          )}
        </div>
      </div>
      <div id={bodyId} className="workspace-group-body" hidden={collapsed}>
        {group.agents.length ? (
          <div className="agent-cards">
            {group.agents.map((item) => (
              <AgentCard
                key={item.name}
                item={item}
                sessions={sessions}
                running={running}
                onAgent={onAgent}
              />
            ))}
          </div>
        ) : (
          <div className="empty-card compact workspace-group-empty">
            <p>{t("workspace.groupNoAgents")}</p>
          </div>
        )}
      </div>
    </article>
  );
}

export default function WorkspaceOverview({
  section,
  agents,
  agent,
  sessions,
  running,
  groups,
  groupActions,
  onAgent,
  onSession,
  onNewSession,
  onConfigure,
  onCreate,
}: WorkspaceOverviewProps) {
  const { t } = useI18n();
  const subagents = section === "subagents";
  // 折叠状态存 group.path；默认全展开，path 不在集合里即展开。
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(
    () => new Set(),
  );
  const toggleGroup = (path: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };
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

      {/* 平铺模式：subagents 分区与单 Agent 视图（groups 传 null），渲染行为与既往一致。 */}
      {!agent && groups === null && (
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
            {agents.map((item) => (
              <AgentCard
                key={item.name}
                item={item}
                sessions={sessions}
                running={running}
                onAgent={onAgent}
              />
            ))}
          </div>
          {!agents.length && (
            <EmptyAgentsCard subagents={subagents} onCreate={onCreate} />
          )}
        </section>
      )}

      {/* 分组模式：多主 Agent 概览；组本身是一级结构，不再渲染「Your Agents」标题。 */}
      {!agent && groups !== null &&
        (groups.length ? (
          <div className="workspace-groups">
            {groups.map((group) => (
              <AgentGroup
                key={group.path}
                group={group}
                collapsed={collapsedGroups.has(group.path)}
                onToggle={toggleGroup}
                sessions={sessions}
                running={running}
                onAgent={onAgent}
                actions={groupActions}
              />
            ))}
          </div>
        ) : (
          !agents.length && (
            <section className="page-section">
              <EmptyAgentsCard subagents={false} onCreate={onCreate} />
            </section>
          )
        ))}

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
