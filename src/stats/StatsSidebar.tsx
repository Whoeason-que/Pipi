// Agent 统计侧栏：只负责选 Agent 与过滤，不在侧栏里逐个拉统计。
import { useMemo, useState } from "react";
import AgentAvatar from "../components/AgentAvatar";
import { useI18n } from "../i18n";
import { IconClose, IconSearch } from "../icons";
import type { AgentDefinition, SessionSummaryView } from "../types";

interface StatsSidebarProps {
  agents: AgentDefinition[];
  selected: string | null;
  sessionsByAgent: Record<string, SessionSummaryView[]>;
  onSelect: (name: string) => void;
}

export function StatsSidebar({ agents, selected, sessionsByAgent, onSelect }: StatsSidebarProps) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const primaryAgents = useMemo(() => agents.filter((agent) => !agent.subagent), [agents]);
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return primaryAgents;
    return primaryAgents.filter((agent) =>
      `${agent.name} ${agent.description}`.toLowerCase().includes(needle),
    );
  }, [primaryAgents, query]);

  return (
    <>
      <div className="sb-filter">
        <div className="search">
          <IconSearch />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t("stats.searchAgents")}
            aria-label={t("stats.searchAgents")}
            autoComplete="off"
            spellCheck={false}
          />
          {query && (
            <button type="button" className="search-clear" title={t("common.search")} onClick={() => setQuery("")}>
              <IconClose />
            </button>
          )}
        </div>
      </div>

      <div className="caption sb-sec stats-sidebar-heading">
        <span>{t("stats.agents")}</span>
        <span className="spacer" />
        <span className="caption sb-count">{primaryAgents.length}</span>
      </div>

      {primaryAgents.length === 0 && <div className="session pending">{t("stats.noAgents")}</div>}

      <nav className="stats-agent-list" aria-label={t("stats.agents")}>
        {filtered.map((agent) => {
          const sessions = sessionsByAgent[agent.name]?.length ?? 0;
          const active = selected === agent.name;
          return (
            <button
              key={agent.name}
              type="button"
              className={`search-item stats-agent-item${active ? " active" : ""}`}
              title={agent.description || agent.name}
              aria-current={active ? "true" : undefined}
              onClick={() => onSelect(agent.name)}
            >
              <AgentAvatar
                name={agent.name}
                subagent={agent.subagent}
                className="stats-agent-avatar"
              />
              <span className="stats-agent-body">
                <span className="stats-agent-top">
                  <span className="truncate stats-agent-name">{agent.name}</span>
                  <span className="stats-agent-count">
                    {t(
                      sessions === 1
                        ? "common.sessionCountOne"
                        : "common.sessionCount",
                      { count: sessions },
                    )}
                  </span>
                </span>
                <span className="truncate stats-agent-desc">
                  {agent.description || t("stats.agentFallback")}
                </span>
              </span>
            </button>
          );
        })}
      </nav>
      {primaryAgents.length > 0 && filtered.length === 0 && (
        <div className="session pending">{t("stats.noMatchingAgents")}</div>
      )}
    </>
  );
}
