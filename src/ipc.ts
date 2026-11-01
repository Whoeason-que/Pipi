import type { IconTheme, IconThemeList } from "./files/icon-theme-types";
import type { WorkspaceListing, WorkspaceFile, WorkspaceChanges, WorkspaceDiff } from "./files/types";
import type { NotificationEnvelope } from "./notifications";
import type { UserImage } from "./types";
/** 宿主命令契约。Tauri/Web 的命令名与参数由 ipc-contract.test.ts 核对。 */
import type { ModelCatalog } from "./catalog";
import type { AgentEventPayload, MessageView, SessionErrorPayload, SessionStatsPayload, SessionSwitchedPayload } from "./chat-runtime";
import type { AgentStatsView, PromptQueueEnvelope, RemoteAccessConfig, RemoteAccessStatus, AgentDefinition, AgentEditableFiles, AgentSkills, ForumThreadView, ModelConfig, PermissionsConfig, PostView, SearchHitView, SessionChangedPayload, SessionImportReportView, SessionInfoView, SessionSnapshotView, SessionStatsView, SessionSummaryView, Settings, SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify, ThreadSummaryView, ThreadView } from "./types";

export interface BackgroundTaskSnapshot {
  task: {
    jobId: string; kind: "shell" | "agent"; agentName: string; sessionId: string; runId: number;
    command: string; cwd: string; targetAgent?: string; childSessionId?: string;
    status: "queued" | "running" | "completed" | "failed" | "terminated" | "orphaned";
    startedAt: number; completedAt?: number; exitCode?: number; outputCursor: number;
    outputTruncated: boolean; error?: string; result?: string; subscribe: boolean;
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
  read_notebook: { args: { agentName: string; sessionId: string }; result: string };
  list_workspace_files: { args: { agentName: string; path: string }; result: WorkspaceListing };
  preview_workspace_file: { args: { agentName: string; path: string }; result: WorkspaceFile };
  workspace_changes: { args: { agentName: string }; result: WorkspaceChanges };
  workspace_diff: { args: { agentName: string; path: string }; result: WorkspaceDiff };
  list_icon_themes: { args: { }; result: IconThemeList };
  get_icon_theme: { args: { }; result: IconTheme | null };
  set_icon_theme: { args: { id?: string | null }; result: IconTheme | null };
  get_settings: { args: {  }; result: Settings };
  save_settings: { args: { settings: Settings }; result: void };
  web_push_public_key: { args: { }; result: string };
  register_push_subscription: { args: { endpoint: string; p256dh: string; auth: string; origin: string }; result: void };
  remove_push_subscription: { args: { endpoint: string }; result: void };
  push_subscription_status: { args: { endpoint: string }; result: boolean };
  test_push_notification: { args: { endpoint: string }; result: void };
  test_desktop_notification: { args: { }; result: void };
  remote_access_status: { args: { }; result: RemoteAccessStatus };
  configure_remote_access: { args: { config: RemoteAccessConfig }; result: RemoteAccessStatus };
  set_remote_access_enabled: { args: { enabled: boolean }; result: RemoteAccessStatus };
  remote_access_token: { args: { rotate: boolean }; result: string };
  list_sessions: { args: { agentName: string; archived?: boolean | null }; result: SessionSummaryView[] };
  open_session: { args: { agentName: string; sessionId: string }; result: SessionSnapshotView };
  session_info: { args: { agentName: string; sessionId: string }; result: SessionInfoView | null };
  session_infos: { args: {  }; result: SessionInfoView[] };
  open_workbench_session: { args: { agentName: string; reset?: boolean | null }; result: SessionInfoView };
  stop_run: { args: { agentName: string; sessionId: string; runId?: number | null }; result: void };
  query_background_tasks: { args: { agentName: string; sessionId: string; jobId?: string | null; afterSeq?: number | null; waitMs?: number | null; includeCompleted?: boolean | null; limit?: number | null }; result: BackgroundTaskSnapshot[] };
  manage_background_task: { args: { agentName: string; sessionId: string; jobId: string; action: string; data?: string | null }; result: BackgroundTaskSnapshot };
  new_session: { args: { agentName: string }; result: void };
  create_session: { args: { agentName: string }; result: SessionInfoView };
  send_prompt: { args: { agentName: string; sessionId?: string | null; prompt: string; model?: ModelConfig | null; images?: UserImage[] | null }; result: SessionInfoView };
  compact_now: { args: { agentName: string; sessionId: string }; result: void };
  steer: { args: { agentName: string; sessionId: string; message: string; images?: UserImage[] | null }; result: void };
  queue_prompt: { args: { agentName: string; sessionId: string; messageId: string; message: string; sendAt?: number | null; images?: UserImage[] | null }; result: PromptQueueEnvelope };
  cancel_queued_prompt: { args: { agentName: string; sessionId: string; messageId: string }; result: PromptQueueEnvelope };
  set_prompt_queue_paused: { args: { agentName: string; sessionId: string; paused: boolean }; result: PromptQueueEnvelope };
  set_session_model: { args: { agentName: string; sessionId: string; model?: ModelConfig | null }; result: void };
  session_messages: { args: { agentName: string; sessionId: string }; result: MessageView[] };
  session_stats: { args: { agentName: string; sessionId: string }; result: SessionStatsView };
  agent_stats: { args: { agentName: string; rangeDays?: number | null }; result: AgentStatsView };
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
  forum_threads: { args: { status?: string | null; tag?: string | null; archived?: boolean | null; limit?: number | null }; result: ThreadSummaryView[] };
  forum_read: { args: { threadId: string; offset?: number | null; limit?: number | null }; result: ForumThreadView };
  forum_search: { args: { query: string; archived?: boolean | null; limit?: number | null }; result: SearchHitView[] };
  forum_post: { args: { title: string; body: string; tags?: string[] | null; mentions?: string[] | null; author?: string | null }; result: ThreadView };
  forum_reply: { args: { threadId: string; body: string; replyTo?: string | null; mentions?: string[] | null; author?: string | null }; result: PostView };
}

export type CommandName = keyof CommandMap;
export type CommandResult<K extends CommandName> = CommandMap[K]["result"];
export type CommandArgs<K extends CommandName> = {} extends CommandMap[K]["args"]
  ? [args?: CommandMap[K]["args"]] : [args: CommandMap[K]["args"]];
export type Invoke = <K extends CommandName>(command: K, ...args: CommandArgs<K>) => Promise<CommandResult<K>>;

export interface EventMap {
  "prompt-queue": PromptQueueEnvelope;
  "notification": NotificationEnvelope;
  "notification-open": NotificationEnvelope;
  "agent-event": AgentEventPayload;
  "session-stats": SessionStatsPayload;
  "session-error": SessionErrorPayload;
  "session-switched": SessionSwitchedPayload;
  "session-changed": SessionChangedPayload;
}
export type Listen = <K extends keyof EventMap>(event: K, listener: (event: { payload: EventMap[K] }) => void) => Promise<() => void>;
