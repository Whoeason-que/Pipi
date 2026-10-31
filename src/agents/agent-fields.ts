import type { PermissionsConfig, SandboxMode } from "../types";

export const DEFAULT_TOOLS = ["read", "write", "edit", "bash", "memory", "notebook"] as const;

/**
 * 2026-10-31 退役：搜索统一走 bash（`rg` 优先，`grep -rn` 兜底）。
 * 旧 agent.json 的 `permissions.tools` 里可能还留着这两个名字；它们在 UI 里
 * 没有开关，保存时由 `permissionsFromFields` 剔除（一次性迁移）。
 */
export const RETIRED_TOOLS = ["glob", "grep"] as const;

export const KNOWN_TOOLS = [
  ...DEFAULT_TOOLS,
  "create_agent",
  "run_agent",
  "read_agent",
  "submit_background_task",
  "query_background_tasks",
  "manage_background_task",
  "session_search",
  "forum_list",
  "forum_read",
  "forum_post",
  "forum_reply",
] as const;


export function permissionsFromFields(tools: string[], sandbox: SandboxMode): PermissionsConfig {
  return {
    tools: tools.filter(
      (tool) => !(RETIRED_TOOLS as readonly string[]).includes(tool),
    ),
    sandbox,
  };
}
