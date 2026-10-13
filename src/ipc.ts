/** 宿主命令契约。Tauri/Web 的命令名与参数由 ipc-contract.test.ts 核对。 */
import type { ModelCatalog } from "./catalog";
import type { AgentEventPayload, ApprovalRequestPayload, MessageView, SessionErrorPayload, SessionStatsPayload, SessionSwitchedPayload } from "./chat-runtime";
import type { AgentDefinition, AgentEditableFiles, AgentSkills, ModelConfig, PermissionsConfig, SessionChangedPayload, SessionImportReportView, SessionInfoView, SessionSnapshotView, SessionStatsView, SessionSummaryView, Settings, SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify } from "./types";

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
  list_agents: { args: { archived?: boolean | null }; result: AgentDefinition[] };
  model_catalog: { args: { refresh?: boolean | null }; result: ModelCatalog };
  create_agent: { args: { name: string; description: string; workspace?: string | null; permissions?: PermissionsConfig | null; model?: string | null; provider?: ModelConfig | null }; result: AgentDefinition };
  save_agent: { args: { def: AgentDefinition }; result: void };
  list_agent_files: { args: { agentName: string }; result: AgentEditableFiles };
  read_agent_file: { args: { agentName: string; relPath: string }; result: string };
  write_agent_file: { args: { agentName: string; relPath: string; content: string }; result: void };
  get_settings: { args: {  }; result: Settings };
  save_settings: { args: { settings: Settings }; result: void };
  list_sessions: { args: { agentName: string; archived?: boolean | null }; result: SessionSummaryView[] };
  open_session: { args: { agentName: string; sessionId: string }; result: SessionSnapshotView };
  session_info: { args: { agentName: string; sessionId: string }; result: SessionInfoView | null };
  session_infos: { args: {  }; result: SessionInfoView[] };
  open_workbench_session: { args: { agentName: string; reset?: boolean | null }; result: SessionInfoView };
  stop_run: { args: { agentName: string; sessionId: string }; result: void };
  query_background_tasks: { args: { agentName: string; sessionId: string; jobId?: string | null; afterSeq?: number | null; waitMs?: number | null; includeCompleted?: boolean | null; limit?: number | null }; result: BackgroundTaskSnapshot[] };
  manage_background_task: { args: { agentName: string; sessionId: string; jobId: string; action: string; data?: string | null }; result: BackgroundTaskSnapshot };
  new_session: { args: { agentName: string }; result: void };
  send_prompt: { args: { agentName: string; sessionId?: string | null; prompt: string; model?: ModelConfig | null }; result: SessionInfoView };
  compact_now: { args: { agentName: string; sessionId: string }; result: void };
  steer: { args: { agentName: string; sessionId: string; message: string }; result: void };
  resolve_approval: { args: { requestId: string; decision: string }; result: void };
  set_session_model: { args: { agentName: string; sessionId: string; model?: ModelConfig | null }; result: void };
  session_messages: { args: { agentName: string; sessionId: string }; result: MessageView[] };
  session_stats: { args: { agentName: string; sessionId: string }; result: SessionStatsView };
  fork_session: { args: { agentName: string; sessionId: string; upToEntryId?: string | null }; result: SessionInfoView };
  archive_agent: { args: { name: string }; result: void };
  restore_agent: { args: { name: string }; result: void };
  delete_agent: { args: { name: string; archived?: boolean | null }; result: void };
  import_sessions: { args: { agentName: string }; result: SessionImportReportView };
  archive_session: { args: { agentName: string; sessionId: string }; result: void };
  restore_session: { args: { agentName: string; sessionId: string }; result: void };
  delete_session: { args: { agentName: string; sessionId: string; archived?: boolean | null }; result: void };
  skill_library: { args: {  }; result: SkillLibrary };
  agent_skills: { args: { agentName: string }; result: AgentSkills };
  set_skill_enabled: { args: { agentName: string; name: string; enabled: boolean }; result: AgentSkills };
  verify_store_skills: { args: { names?: string[] | null }; result: SkillVerify[] };
  install_skill: { args: { source: string; skills?: string[] | null; all: boolean; dryRun: boolean }; result: SkillInstallReport };
  remove_store_skill: { args: { name: string }; result: number };
  sync_agent_skills: { args: { agentName: string; repair: boolean }; result: SkillSyncReport };
  skill_usage: { args: { name: string }; result: SkillUsage[] };
}

export type CommandName = keyof CommandMap;
export type CommandResult<K extends CommandName> = CommandMap[K]["result"];
export type CommandArgs<K extends CommandName> = {} extends CommandMap[K]["args"]
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
