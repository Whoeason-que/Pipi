import type { PermissionsConfig, SandboxMode } from "../types";

export const DEFAULT_TOOLS = ["read", "write", "edit", "bash", "memory", "glob", "grep"] as const;
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
  return { tools, sandbox };
}
