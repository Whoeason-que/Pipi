import type { BashMode, PermissionsConfig, SandboxMode } from "../types";
export const BASH_MODE_LABELS: Record<BashMode, string> = {
  allowAll: "全部允许",
  allowlist: "白名单",
  denylist: "黑名单",
};

export const DEFAULT_TOOLS = ["read", "write", "edit", "bash", "memory", "glob", "grep"] as const;
export const KNOWN_TOOLS = [
  ...DEFAULT_TOOLS,
  "create_agent",
  "run_agent",
  "read_agent",
  "submit_background_task",
  "query_background_tasks",
  "manage_background_task",
] as const;


export function permissionsFromFields(tools: string[], mode: BashMode, commands: string, sandbox: SandboxMode): PermissionsConfig {
  return { tools, bash: { mode, commands: mode === "allowAll" ? [] : commands.split("\n").map(command => command.trim()).filter(Boolean) }, sandbox };
}
