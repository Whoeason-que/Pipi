/** 宿主命令契约。Tauri/Web 的命令名与参数由 ipc-contract.test.ts 核对。 */
import type { ModelCatalog } from "./catalog";
import type { AgentEventPayload, ApprovalRequestPayload, MessageView, SessionErrorPayload, SessionStatsPayload, SessionSwitchedPayload } from "./chat-runtime";
import type { AgentDefinition, ModelConfig, PermissionsConfig, SessionChangedPayload, SessionInfoView, SessionStatsView, SessionSummaryView, Settings } from "./types";

export interface BackgroundTaskSnapshot {
  task: {
    jobId: string; kind: "shell" | "agent"; agentName: string; sessionId: string; runId: number;
    command: string; cwd: string; targetAgent?: string; childSessionId?: string;
    status: "queued" | "running" | "completed" | "failed" | "terminated" | "orphaned";
    startedAt: number; completedAt?: number; exitCode?: number; outputCursor: number;
    outputTruncated: boolean; error?: string; result?: string;
  };
  output: Array<{ seq: number; stream: string; text: string; timestamp: number }>;
  nextSeq: number;
}

export interface CommandMap {
  list_agents: { args: {  }; result: AgentDefinition[] };
  model_catalog: { args: { refresh?: boolean | null }; result: ModelCatalog };
  create_agent: { args: { name: string; description: string; workspace?: string | null; permissions?: PermissionsConfig | null; model?: string | null; provider?: ModelConfig | null }; result: AgentDefinition };
  load_agent: { args: { name: string }; result: AgentDefinition };
  save_agent: { args: { def: AgentDefinition }; result: void };
  list_archived_agents: { args: {  }; result: AgentDefinition[] };
  list_agent_files: { args: { agentName: string }; result: string[] };
  read_agent_file: { args: { agentName: string; relPath: string }; result: string };
  write_agent_file: { args: { agentName: string; relPath: string; content: string }; result: void };
  get_settings: { args: {  }; result: Settings };
  save_settings: { args: { settings: Settings }; result: void };
  list_sessions: { args: { agentName: string }; result: SessionSummaryView[] };
  open_session: { args: { agentName: string; sessionId: string }; result: void };
  session_info: { args: { agentName: string; sessionId: string }; result: SessionInfoView | null };
  session_infos: { args: {  }; result: SessionInfoView[] };
  ensure_test_session: { args: { agentName: string }; result: SessionInfoView };
  reset_test_session: { args: { agentName: string }; result: SessionInfoView };
  session_running: { args: { agentName: string; sessionId: string }; result: boolean };
  stop_run: { args: { agentName: string; sessionId: string }; result: void };
  query_background_tasks: { args: { agentName: string; sessionId: string; jobId?: string | null; afterSeq?: number | null; waitMs?: number | null; includeCompleted?: boolean | null; limit?: number | null }; result: BackgroundTaskSnapshot[] };
  manage_background_task: { args: { agentName: string; sessionId: string; jobId: string; action: string; data?: string | null }; result: BackgroundTaskSnapshot };
  new_session: { args: { agentName: string }; result: void };
  send_prompt: { args: { agentName: string; sessionId?: string | null; prompt: string; model?: ModelConfig | null }; result: void };
  compact_now: { args: { agentName: string; sessionId: string }; result: void };
  steer: { args: { agentName: string; sessionId: string; message: string }; result: void };
  resolve_approval: { args: { requestId: string; decision: string }; result: void };
  set_session_model: { args: { agentName: string; sessionId: string; model?: ModelConfig | null }; result: void };
  session_messages: { args: { agentName: string; sessionId: string }; result: MessageView[] };
  session_stats: { args: { agentName: string; sessionId: string }; result: SessionStatsView };
  fork_session: { args: { agentName: string; sessionId: string; upToEntryId?: string | null }; result: SessionInfoView };
  archive_agent: { args: { name: string }; result: void };
  restore_agent: { args: { name: string }; result: void };
  delete_agent: { args: { name: string }; result: void };
  delete_archived_agent: { args: { name: string }; result: void };
  list_archived_sessions: { args: { agentName: string }; result: SessionSummaryView[] };
  archive_session: { args: { agentName: string; sessionId: string }; result: void };
  restore_session: { args: { agentName: string; sessionId: string }; result: void };
  delete_session: { args: { agentName: string; sessionId: string }; result: void };
  delete_archived_session: { args: { agentName: string; sessionId: string }; result: void };
}

export type CommandName = keyof CommandMap;
export type CommandResult<K extends CommandName> = CommandMap[K]["result"];
export type CommandArgs<K extends CommandName> = keyof CommandMap[K]["args"] extends never
  ? [args?: CommandMap[K]["args"]] : [args: CommandMap[K]["args"]];
export type Invoke = <K extends CommandName>(command: K, ...args: CommandArgs<K>) => Promise<CommandResult<K>>;

export interface EventMap {
  "agent-event": AgentEventPayload;
  "session-stats": SessionStatsPayload;
  "session-error": SessionErrorPayload;
  "approval-request": ApprovalRequestPayload;
  "session-switched": SessionSwitchedPayload;
  "session-changed": SessionChangedPayload;
}
export type Listen = <K extends keyof EventMap>(event: K, listener: (event: { payload: EventMap[K] }) => void) => Promise<() => void>;
