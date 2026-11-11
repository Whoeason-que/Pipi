import { useMemo, useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import {
  normalizeWorkspacePath,
  resolveWorkspaceTarget,
  type WorkspaceInfo,
} from "../app/workspaces";
import { useI18n } from "../i18n";
import type { AgentDefinition } from "../types";
import { TerminalPanel } from "./TerminalPanel";

/**
 * Navigation-page terminal: pick a workspace, then work in its shells. The
 * left app sidebar keeps the Agent list; workspace selection lives here so the
 * page is useful even when the current Agent sits in a different directory.
 */
export function TerminalView({
  workspaces,
  activeWorkspace,
  current,
  agents,
}: {
  workspaces: WorkspaceInfo[];
  activeWorkspace: string;
  current: AgentDefinition | null;
  agents: AgentDefinition[];
}) {
  const { t } = useI18n();
  const [preview, setPreview] = useState<string | null>(null);
  const target = useMemo(
    () =>
      resolveWorkspaceTarget(
        preview,
        activeWorkspace,
        current?.workspace,
        workspaces,
      ),
    [preview, activeWorkspace, current?.workspace, workspaces],
  );
  const anchor = useMemo(() => {
    if (!target) return current ?? agents[0] ?? null;
    const path = normalizeWorkspacePath(target.path);
    return (
      agents.find(
        (agent) =>
          !agent.subagent &&
          normalizeWorkspacePath(agent.workspace) === path,
      ) ??
      current ??
      agents[0] ??
      null
    );
  }, [target, agents, current]);

  return (
    <div className="terminal-workbench">
      <header className="workspace-files-toolbar terminal-page-toolbar">
        <NavigationToggle />
        <strong className="terminal-page-title">{t("terminal.title")}</strong>
        {workspaces.length > 0 && (
          <label className="terminal-workspace-picker">
            <span>{t("terminal.workspace")}</span>
            <select
              className="text-input"
              aria-label={t("terminal.workspace")}
              value={target?.path ?? ""}
              onChange={(event) => setPreview(event.target.value)}
            >
              {workspaces.map((workspace) => (
                <option key={workspace.path} value={workspace.path}>
                  {workspace.name}
                </option>
              ))}
            </select>
          </label>
        )}
      </header>
      <div className="terminal-page-body">
        {anchor ? (
          <TerminalPanel
            key={target?.path ?? `agent:${anchor.name}`}
            agentName={anchor.name}
            workspace={target?.path ?? null}
            active
            variant="page"
          />
        ) : (
          <div className="empty-state terminal-empty">
            <strong>{t("terminal.unavailable")}</strong>
          </div>
        )}
      </div>
    </div>
  );
}
