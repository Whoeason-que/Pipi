//! 宿主命令的单一事实来源。
//!
//! 全部命令的名称（[`COMMANDS`]）、参数结构体、逻辑与 JSON 分发表都在这里；
//! `crates/pipi`（桌面壳）与 `server` 特性里的 Web 宿主只做 transport：前者把
//! 每个命令转发成 `#[tauri::command]` 薄壳，后者调用 [`dispatch`]。两边不再各自
//! 复制命令表，也不再手写 JSON 参数解析。
//!
//! 参数结构体统一 `#[serde(rename_all = "camelCase")]`，字段名、类型与可选性
//! 就是前端 `src/ipc.ts` 的宿主命令契约（由 `tests/ipc-contract.test.ts` 核对）。

use crate::remote::{CommandSource, RemoteAccessConfig, RemoteAccessStatus};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::{self, Settings};
use crate::tools::browser::{BrowserBounds, BrowserViewState};
use crate::workspace::{WorkspaceChanges, WorkspaceDiff, WorkspaceFile, WorkspaceListing};
use pipi_agent::stats::SessionStats;
use pipi_protocol::forum::{
    ForumRepo, NewPost, NewThread, Post, PostRange, PostVia, SearchHit, SearchOptions, Thread,
    ThreadFilter, ThreadStatus, ThreadSummary,
};
use pipi_protocol::permissions::PermissionsConfig;
use pipi_protocol::session::{SessionCommit, SessionImportReport, SessionSummary};
use pipi_protocol::{BackgroundTaskSnapshot, Message, Model};
use pipi_provider::catalog::ModelCatalog;
use pipi_terminal::TerminalSnapshot;
use pipi_venv::agents::{self, AgentDefinition, AgentEditableFiles};
use pipi_venv::skills::{
    AgentSkills, SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify,
};

use crate::events::{EventEmitter, PromptQueueEnvelope};
use crate::runtime::{self, RuntimeState, SessionInfo, SessionSnapshot, agent_repo};
use crate::terminal::TerminalReadView;

/// 全部宿主命令名，顺序与前端 `src/ipc.ts` 的 `CommandMap` 一致。
pub const COMMANDS: &[&str] = &[
    "list_agents",
    "model_catalog",
    "create_agent",
    "save_agent",
    "list_agent_files",
    "read_agent_file",
    "write_agent_file",
    "read_notebook",
    "mcp_status",
    "list_workspace_files",
    "preview_workspace_file",
    "workspace_changes",
    "workspace_diff",
    "list_icon_themes",
    "get_icon_theme",
    "set_icon_theme",
    "get_settings",
    "save_settings",
    "web_push_public_key",
    "register_push_subscription",
    "remove_push_subscription",
    "push_subscription_status",
    "test_push_notification",
    "test_desktop_notification",
    "remote_access_status",
    "configure_remote_access",
    "set_remote_access_enabled",
    "remote_access_token",
    "list_sessions",
    "open_session",
    "session_info",
    "session_infos",
    "answer_question",
    "answer_approval",
    "open_workbench_session",
    "stop_run",
    "query_background_tasks",
    "manage_background_task",
    "new_session",
    "create_session",
    "browser_view",
    "list_terminals",
    "open_terminal",
    "read_terminal",
    "write_terminal",
    "resize_terminal",
    "close_terminal",
    "send_prompt",
    "compact_now",
    "steer",
    "queue_prompt",
    "cancel_queued_prompt",
    "set_prompt_queue_paused",
    "set_session_model",
    "set_session_permission_mode",
    "session_messages",
    "session_history",
    "session_stats",
    "agent_stats",
    "fork_session",
    "archive_agent",
    "restore_agent",
    "delete_agent",
    "import_sessions",
    "archive_session",
    "restore_session",
    "delete_session",
    "skill_library",
    "agent_skills",
    "set_skill_enabled",
    "verify_store_skills",
    "install_skill",
    "remove_store_skill",
    "sync_agent_skills",
    "skill_usage",
    "forum_threads",
    "forum_read",
    "forum_search",
    "forum_post",
    "forum_reply",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetIconThemeArgs {
    pub id: Option<String>,
}

pub async fn list_icon_themes() -> Result<crate::icon_themes::IconThemeList, String> {
    tokio::task::spawn_blocking(crate::icon_themes::list)
        .await
        .map_err(|e| e.to_string())
}
pub async fn get_icon_theme() -> Result<Option<Value>, String> {
    tokio::task::spawn_blocking(crate::icon_themes::current)
        .await
        .map_err(|e| e.to_string())?
}
pub async fn set_icon_theme(args: SetIconThemeArgs) -> Result<Option<Value>, String> {
    tokio::task::spawn_blocking(move || crate::icon_themes::select(args.id))
        .await
        .map_err(|e| e.to_string())?
}

// ============ Agent ============

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsArgs {
    pub archived: Option<bool>,
}

/// 列出活跃或归档的 Agent。活跃 Agent 在首次启动时自动播种默认 Agent（Pipi）。
pub fn list_agents(args: ListAgentsArgs) -> Result<Vec<AgentDefinition>, String> {
    if args.archived.unwrap_or(false) {
        agents::list_archived_agents()
    } else {
        agents::list_agents_bootstrapped()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogArgs {
    pub refresh: Option<bool>,
}

/// 模型目录（models.dev + 本地缓存）。
pub async fn model_catalog(args: ModelCatalogArgs) -> Result<ModelCatalog, String> {
    pipi_provider::catalog::load_catalog(
        args.refresh.unwrap_or(false),
        pipi_venv::av::paths::catalog_cache_path(),
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentArgs {
    pub name: String,
    pub description: String,
    pub workspace: Option<String>,
    pub permissions: Option<PermissionsConfig>,
    pub model: Option<String>,
    pub provider: Option<Model>,
}

pub fn create_agent(args: CreateAgentArgs) -> Result<AgentDefinition, String> {
    let CreateAgentArgs {
        name,
        description,
        workspace,
        permissions,
        model,
        provider,
    } = args;
    agents::create_agent(
        &name,
        &description,
        workspace.as_deref(),
        permissions,
        model.as_deref(),
        provider,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentArgs {
    pub def: AgentDefinition,
}

pub fn save_agent(runtime: &RuntimeState, args: SaveAgentArgs) -> Result<(), String> {
    runtime.save_agent_definition(&args.def)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentFilesArgs {
    pub agent_name: String,
}

/// 列出 Agent 目录内可编辑的 Markdown：契约声明的指令文件 + 记忆目录下的 `.md`
/// （位置来自 agent.toml 的 `[resources]`，不是硬编码目录）。
pub fn list_agent_files(args: ListAgentFilesArgs) -> Result<AgentEditableFiles, String> {
    agents::list_agent_editable_files(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadAgentFileArgs {
    pub agent_name: String,
    pub rel_path: String,
}

/// 读取 Agent 目录内的可编辑 Markdown（harness/AGENTS.md / harness/memory/*.md）。
pub fn read_agent_file(args: ReadAgentFileArgs) -> Result<String, String> {
    agents::read_agent_file(&args.agent_name, &args.rel_path)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteAgentFileArgs {
    pub agent_name: String,
    pub rel_path: String,
    pub content: String,
}

/// 写入 Agent 目录内的可编辑 Markdown（harness/AGENTS.md / harness/memory/*.md）。
pub fn write_agent_file(runtime: &RuntimeState, args: WriteAgentFileArgs) -> Result<(), String> {
    runtime.write_agent_file(&args.agent_name, &args.rel_path, &args.content)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadNotebookArgs {
    pub agent_name: String,
    pub session_id: String,
}

/// 读取会话 notebook（右侧检查器渲染用）。还没有 notebook 时返回空串；
/// 路径由 `pipi-venv` 的 Agent 目录布局推出，不在这里手写 `notebooks/`。
pub fn read_notebook(args: ReadNotebookArgs) -> Result<String, String> {
    let path = agents::notebook_path(&args.agent_name, &args.session_id)
        .ok_or_else(|| format!("Cannot resolve notebook for session {}", args.session_id))?;
    match pipi_protocol::fs::ensure_real_file(&path, "notebook") {
        Ok(true) => std::fs::read_to_string(&path)
            .map_err(|error| format!("Failed to read notebook: {error}")),
        Ok(false) => Ok(String::new()),
        Err(error) => Err(error),
    }
}

// ============ MCP ============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusArgs {
    pub agent_name: String,
}

/// 该 Agent 声明的 MCP 服务器与连接状态（Agent 工作台展示）。
/// 只读：连接由会话启动时的 `prepare` 建立，这里不主动拉起进程。
pub fn mcp_status(
    runtime: &RuntimeState,
    args: McpStatusArgs,
) -> Result<Vec<crate::mcp::McpServerStatus>, String> {
    runtime.mcp_status(&args.agent_name)
}

// ============ 工作区只读预览 ============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListWorkspaceFilesArgs {
    pub agent_name: String,
    pub path: String,
}

pub async fn list_workspace_files(
    args: ListWorkspaceFilesArgs,
) -> Result<WorkspaceListing, String> {
    tokio::task::spawn_blocking(move || crate::workspace::list_files(&args.agent_name, &args.path))
        .await
        .map_err(|e| format!("Failed to read directory: {e}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewWorkspaceFileArgs {
    pub agent_name: String,
    pub path: String,
}

pub async fn preview_workspace_file(
    args: PreviewWorkspaceFileArgs,
) -> Result<WorkspaceFile, String> {
    tokio::task::spawn_blocking(move || {
        crate::workspace::preview_file(&args.agent_name, &args.path)
    })
    .await
    .map_err(|e| format!("Failed to read file: {e}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangesArgs {
    pub agent_name: String,
}

pub async fn workspace_changes(args: WorkspaceChangesArgs) -> Result<WorkspaceChanges, String> {
    crate::workspace::changes(&args.agent_name).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDiffArgs {
    pub agent_name: String,
    pub path: String,
}

pub async fn workspace_diff(args: WorkspaceDiffArgs) -> Result<WorkspaceDiff, String> {
    crate::workspace::diff(&args.agent_name, &args.path).await
}

pub fn get_settings() -> Settings {
    settings::load_settings()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsArgs {
    pub settings: Settings,
}

pub fn save_settings(args: SaveSettingsArgs) -> Result<(), String> {
    settings::save_settings(&args.settings)
}

// ============ 桌面远程访问管理 ============

pub async fn web_push_public_key(runtime: &RuntimeState) -> Result<String, String> {
    #[cfg(feature = "server")]
    {
        let service = runtime.push_service();
        tokio::task::spawn_blocking(move || service.public_key())
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = runtime;
        Err("Web Push requires the server feature".into())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterPushSubscriptionArgs {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub origin: String,
}

pub async fn register_push_subscription(
    runtime: &RuntimeState,
    args: RegisterPushSubscriptionArgs,
) -> Result<(), String> {
    #[cfg(feature = "server")]
    {
        let service = runtime.push_service();
        tokio::task::spawn_blocking(move || {
            service.subscribe(crate::notifications::push::PushSubscription {
                endpoint: args.endpoint,
                p256dh: args.p256dh,
                auth: args.auth,
                origin: args.origin,
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (runtime, args);
        Err("Web Push requires the server feature".into())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovePushSubscriptionArgs {
    pub endpoint: String,
}

pub async fn remove_push_subscription(
    runtime: &RuntimeState,
    args: RemovePushSubscriptionArgs,
) -> Result<(), String> {
    #[cfg(feature = "server")]
    {
        let service = runtime.push_service();
        tokio::task::spawn_blocking(move || service.unsubscribe(&args.endpoint))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (runtime, args);
        Err("Web Push requires the server feature".into())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscriptionStatusArgs {
    pub endpoint: String,
}

pub async fn push_subscription_status(
    runtime: &RuntimeState,
    args: PushSubscriptionStatusArgs,
) -> Result<bool, String> {
    #[cfg(feature = "server")]
    {
        let service = runtime.push_service();
        tokio::task::spawn_blocking(move || service.contains(&args.endpoint))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (runtime, args);
        Err("Web Push requires the server feature".into())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestPushNotificationArgs {
    pub endpoint: String,
}

pub async fn test_push_notification(
    runtime: &RuntimeState,
    args: TestPushNotificationArgs,
) -> Result<(), String> {
    #[cfg(feature = "server")]
    {
        runtime
            .push_service()
            .deliver(
                crate::notifications::NotificationEnvelope::new(
                    "Pipi",
                    "",
                    0,
                    crate::notifications::NotificationKind::Completed,
                ),
                Some(args.endpoint),
            )
            .await
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (runtime, args);
        Err("Web Push requires the server feature".into())
    }
}

pub fn test_desktop_notification(
    runtime: &RuntimeState,
    source: CommandSource,
) -> Result<(), String> {
    source.require_desktop()?;
    runtime.events().test_desktop_notification();
    Ok(())
}

pub async fn remote_access_status(
    runtime: &RuntimeState,
    source: CommandSource,
) -> Result<RemoteAccessStatus, String> {
    source.require_desktop()?;
    Ok(runtime.remote_access()?.status().await)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureRemoteAccessArgs {
    pub config: RemoteAccessConfig,
}

pub async fn configure_remote_access(
    runtime: &RuntimeState,
    source: CommandSource,
    args: ConfigureRemoteAccessArgs,
) -> Result<RemoteAccessStatus, String> {
    source.require_desktop()?;
    runtime.remote_access()?.configure(args.config).await
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRemoteAccessEnabledArgs {
    pub enabled: bool,
}

pub async fn set_remote_access_enabled(
    runtime: &RuntimeState,
    source: CommandSource,
    args: SetRemoteAccessEnabledArgs,
) -> Result<RemoteAccessStatus, String> {
    source.require_desktop()?;
    runtime.remote_access()?.set_enabled(args.enabled).await
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessTokenArgs {
    pub rotate: bool,
}

pub async fn remote_access_token(
    runtime: &RuntimeState,
    source: CommandSource,
    args: RemoteAccessTokenArgs,
) -> Result<String, String> {
    source.require_desktop()?;
    runtime.remote_access()?.token(args.rotate).await
}

// ============ 会话 ============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsArgs {
    pub agent_name: String,
    pub archived: Option<bool>,
}

pub fn list_sessions(
    runtime: &RuntimeState,
    args: ListSessionsArgs,
) -> Result<Vec<SessionSummary>, String> {
    if args.archived.unwrap_or(false) {
        runtime.list_archived_sessions(&args.agent_name)
    } else {
        runtime::list_sessions(&args.agent_name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn open_session(
    runtime: &RuntimeState,
    args: OpenSessionArgs,
) -> Result<SessionSnapshot, String> {
    runtime.open_session(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn session_info(
    runtime: &RuntimeState,
    args: SessionInfoArgs,
) -> Result<Option<SessionInfo>, String> {
    runtime.session_info(&args.agent_name, &args.session_id)
}

/// 所有打开中的会话：前端据此知道「哪些 Agent 正在跑」（多 Agent 并发下运行态是集合）。
pub fn session_infos(runtime: &RuntimeState) -> Result<Vec<SessionInfo>, String> {
    runtime.session_infos()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerQuestionArgs {
    pub agent_name: String,
    pub session_id: String,
    pub question_id: String,
    pub answer: pipi_protocol::interaction::QuestionAnswer,
}

pub fn answer_question(
    runtime: &RuntimeState,
    args: AnswerQuestionArgs,
) -> Result<crate::events::QuestionsEnvelope, String> {
    runtime.answer_question(
        &args.agent_name,
        &args.session_id,
        &args.question_id,
        args.answer,
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerApprovalArgs {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub approval_id: String,
    pub decision: crate::approvals::ApprovalDecision,
}

pub fn answer_approval(runtime: &RuntimeState, args: AnswerApprovalArgs) -> Result<(), String> {
    runtime.answer_approval(
        &args.agent_name,
        &args.session_id,
        args.run_id,
        &args.approval_id,
        args.decision,
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionPermissionModeArgs {
    pub agent_name: String,
    pub session_id: String,
    pub mode: Option<pipi_protocol::permissions::PermissionMode>,
    pub confirm_full_access: bool,
}

pub fn set_session_permission_mode(
    runtime: &RuntimeState,
    args: SetSessionPermissionModeArgs,
) -> Result<SessionInfo, String> {
    runtime.set_session_permission_mode(
        &args.agent_name,
        &args.session_id,
        args.mode,
        args.confirm_full_access,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWorkbenchSessionArgs {
    pub agent_name: String,
    pub reset: Option<bool>,
}

/// 打开或重置工作台测试会话。
pub fn open_workbench_session(
    runtime: &RuntimeState,
    args: OpenWorkbenchSessionArgs,
) -> Result<SessionInfo, String> {
    if args.reset.unwrap_or(false) {
        runtime.reset_test_session(&args.agent_name)
    } else {
        runtime.ensure_test_session(&args.agent_name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopRunArgs {
    pub agent_name: String,
    pub session_id: String,
    /// 可选：只停止这一轮。超时后迟到的 stop 不会误停下一轮。
    #[serde(default)]
    pub run_id: Option<usize>,
}

pub fn stop_run(runtime: &RuntimeState, args: StopRunArgs) -> Result<(), String> {
    runtime.stop_run_if(&args.agent_name, &args.session_id, args.run_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryBackgroundTasksArgs {
    pub agent_name: String,
    pub session_id: String,
    pub job_id: Option<String>,
    pub after_seq: Option<u64>,
    pub wait_ms: Option<u64>,
    pub include_completed: Option<bool>,
    pub limit: Option<usize>,
}

pub async fn query_background_tasks(
    runtime: &RuntimeState,
    args: QueryBackgroundTasksArgs,
) -> Result<Vec<BackgroundTaskSnapshot>, String> {
    let QueryBackgroundTasksArgs {
        agent_name,
        session_id,
        job_id,
        after_seq,
        wait_ms,
        include_completed,
        limit,
    } = args;
    runtime
        .query_background_tasks(
            &agent_name,
            &session_id,
            crate::tools::background::BackgroundTaskQuery {
                job_id,
                after_seq: after_seq.unwrap_or(0),
                wait_ms: wait_ms.unwrap_or(0),
                include_completed: include_completed.unwrap_or(true),
                limit: limit.unwrap_or(20),
                observe: false,
            },
        )
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManageBackgroundTaskArgs {
    pub agent_name: String,
    pub session_id: String,
    pub job_id: String,
    pub action: String,
    pub data: Option<String>,
}

pub async fn manage_background_task(
    runtime: &RuntimeState,
    args: ManageBackgroundTaskArgs,
) -> Result<BackgroundTaskSnapshot, String> {
    runtime
        .manage_background_task(
            &args.agent_name,
            &args.session_id,
            &args.job_id,
            &args.action,
            args.data,
        )
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionArgs {
    pub agent_name: String,
}

pub fn new_session(runtime: &RuntimeState, args: NewSessionArgs) -> Result<(), String> {
    runtime.new_session(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionArgs {
    pub agent_name: String,
}

pub fn create_session(
    runtime: &RuntimeState,
    args: CreateSessionArgs,
) -> Result<SessionInfo, String> {
    runtime.create_session(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserViewArgs {
    pub agent_name: String,
    pub session_id: String,
    pub action: String,
    pub url: Option<String>,
    pub bounds: Option<BrowserBounds>,
}

pub async fn browser_view(
    runtime: &RuntimeState,
    source: CommandSource,
    args: BrowserViewArgs,
) -> Result<BrowserViewState, String> {
    source.require_desktop()?;
    runtime
        .browser_view(
            &args.agent_name,
            &args.session_id,
            &args.action,
            args.url.as_deref(),
            args.bounds,
        )
        .await
}

// ============ 用户终端 ============
//
// 终端按 canonical workspace 归属，不绑定会话或运行。命令只做参数解析与
// 工作区分辨；PTY 生命周期在 `pipi-terminal`，装配（shell/env）在
// `crate::terminal` 桥。用户终端是 user-controlled spawn，不经过沙箱。

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTerminalsArgs {
    pub agent_name: String,
    pub workspace: Option<String>,
}

pub fn list_terminals(
    runtime: &RuntimeState,
    args: ListTerminalsArgs,
) -> Result<Vec<TerminalSnapshot>, String> {
    let workspace =
        crate::terminal::resolve_workspace(&args.agent_name, args.workspace.as_deref())?;
    Ok(runtime.terminals().list(&workspace))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTerminalArgs {
    pub agent_name: String,
    pub workspace: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

pub async fn open_terminal(
    runtime: &RuntimeState,
    args: OpenTerminalArgs,
) -> Result<TerminalSnapshot, String> {
    let workspace =
        crate::terminal::resolve_workspace(&args.agent_name, args.workspace.as_deref())?;
    let cols = args
        .cols
        .unwrap_or(crate::terminal::DEFAULT_COLS)
        .clamp(20, 500);
    let rows = args
        .rows
        .unwrap_or(crate::terminal::DEFAULT_ROWS)
        .clamp(5, 200);
    let spec = crate::terminal::shell_spec(cols, rows);
    let manager = runtime.terminals().clone();
    // openpty + spawn are blocking; keep them off the reactor (and off the GTK
    // main thread in the desktop shell).
    tokio::task::spawn_blocking(move || manager.open(&workspace, spec))
        .await
        .map_err(|error| format!("Terminal start task failed: {error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadTerminalArgs {
    pub agent_name: String,
    pub workspace: Option<String>,
    pub terminal_id: String,
    pub after_seq: u64,
    pub max_bytes: Option<usize>,
    pub wait_ms: Option<u64>,
}

pub async fn read_terminal(
    runtime: &RuntimeState,
    args: ReadTerminalArgs,
) -> Result<TerminalReadView, String> {
    let workspace =
        crate::terminal::resolve_workspace(&args.agent_name, args.workspace.as_deref())?;
    let max_bytes = args
        .max_bytes
        .unwrap_or(crate::terminal::DEFAULT_READ_BYTES)
        .clamp(1024, crate::terminal::MAX_READ_BYTES);
    let wait_ms = args
        .wait_ms
        .unwrap_or(crate::terminal::DEFAULT_WAIT_MS)
        .min(crate::terminal::MAX_WAIT_MS);
    let read = runtime
        .terminals()
        .read(
            &workspace,
            &args.terminal_id,
            args.after_seq,
            max_bytes,
            wait_ms,
        )
        .await?;
    Ok(read.into())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteTerminalArgs {
    pub agent_name: String,
    pub workspace: Option<String>,
    pub terminal_id: String,
    pub data: String,
}

pub async fn write_terminal(runtime: &RuntimeState, args: WriteTerminalArgs) -> Result<(), String> {
    let workspace =
        crate::terminal::resolve_workspace(&args.agent_name, args.workspace.as_deref())?;
    let data = crate::terminal::decode_input(&args.data)?;
    runtime
        .terminals()
        .write(&workspace, &args.terminal_id, data)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeTerminalArgs {
    pub agent_name: String,
    pub workspace: Option<String>,
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

pub fn resize_terminal(runtime: &RuntimeState, args: ResizeTerminalArgs) -> Result<(), String> {
    let workspace =
        crate::terminal::resolve_workspace(&args.agent_name, args.workspace.as_deref())?;
    runtime
        .terminals()
        .resize(&workspace, &args.terminal_id, args.cols, args.rows)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseTerminalArgs {
    pub agent_name: String,
    pub workspace: Option<String>,
    pub terminal_id: String,
}

pub fn close_terminal(runtime: &RuntimeState, args: CloseTerminalArgs) -> Result<(), String> {
    let workspace =
        crate::terminal::resolve_workspace(&args.agent_name, args.workspace.as_deref())?;
    runtime.terminals().close(&workspace, &args.terminal_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPromptArgs {
    pub agent_name: String,
    pub session_id: Option<String>,
    pub prompt: String,
    pub model: Option<Model>,
    pub images: Option<Vec<pipi_protocol::UserImage>>,
}

pub fn send_prompt(
    runtime: &RuntimeState,
    emitter: EventEmitter,
    args: SendPromptArgs,
) -> Result<SessionInfo, String> {
    runtime.send_prompt_with_images(
        &args.agent_name,
        args.session_id.as_deref(),
        &args.prompt,
        args.model,
        emitter,
        args.images.as_deref().unwrap_or(&[]),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactNowArgs {
    pub agent_name: String,
    pub session_id: String,
    /// `summary`（默认）或 `edit`（Agent 主导的上下文编辑）。
    pub mode: Option<String>,
}

/// 手动压缩当前会话（跳过阈值预检，走与自动压缩相同的分叉/归档路径）。
pub fn compact_now(
    runtime: &RuntimeState,
    emitter: EventEmitter,
    args: CompactNowArgs,
) -> Result<(), String> {
    let mode = match args.mode.as_deref() {
        None | Some("") | Some("summary") => pipi_context::compaction::CompactMode::Summary,
        Some("edit") => pipi_context::compaction::CompactMode::Edit,
        Some(other) => return Err(format!("Unknown compact mode: {other}")),
    };
    runtime.compact_now(&args.agent_name, &args.session_id, mode, emitter)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerArgs {
    pub agent_name: String,
    pub session_id: String,
    pub message: String,
    pub images: Option<Vec<pipi_protocol::UserImage>>,
}

/// 运行中插话（steering）：注入当前运行的下一轮上下文。
pub fn steer(runtime: &RuntimeState, args: SteerArgs) -> Result<(), String> {
    runtime.steer_with_images(
        &args.agent_name,
        &args.session_id,
        &args.message,
        args.images.as_deref().unwrap_or(&[]),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuePromptArgs {
    pub agent_name: String,
    pub session_id: String,
    pub message_id: String,
    pub message: String,
    pub send_at: Option<u64>,
    pub images: Option<Vec<pipi_protocol::UserImage>>,
}

pub fn queue_prompt(
    runtime: &RuntimeState,
    args: QueuePromptArgs,
) -> Result<PromptQueueEnvelope, String> {
    runtime.queue_prompt_with_images(
        &args.agent_name,
        &args.session_id,
        &args.message_id,
        &args.message,
        args.send_at,
        args.images.as_deref().unwrap_or(&[]),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelQueuedPromptArgs {
    pub agent_name: String,
    pub session_id: String,
    pub message_id: String,
}

pub fn cancel_queued_prompt(
    runtime: &RuntimeState,
    args: CancelQueuedPromptArgs,
) -> Result<PromptQueueEnvelope, String> {
    runtime.cancel_queued_prompt(&args.agent_name, &args.session_id, &args.message_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPromptQueuePausedArgs {
    pub agent_name: String,
    pub session_id: String,
    pub paused: bool,
}

pub fn set_prompt_queue_paused(
    runtime: &RuntimeState,
    args: SetPromptQueuePausedArgs,
) -> Result<PromptQueueEnvelope, String> {
    runtime.set_prompt_queue_paused(&args.agent_name, &args.session_id, args.paused)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionModelArgs {
    pub agent_name: String,
    pub session_id: String,
    pub model: Option<Model>,
}

pub fn set_session_model(runtime: &RuntimeState, args: SetSessionModelArgs) -> Result<(), String> {
    runtime.set_session_model(&args.agent_name, &args.session_id, args.model)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessagesArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub async fn session_messages(
    runtime: &RuntimeState,
    args: SessionMessagesArgs,
) -> Result<Vec<Message>, String> {
    runtime
        .session_messages(&args.agent_name, &args.session_id)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryArgs {
    pub agent_name: String,
    pub session_id: String,
}

/// 会话提交历史（只读，右栏 git tree）。
pub fn session_history(
    runtime: &RuntimeState,
    args: SessionHistoryArgs,
) -> Result<Vec<SessionCommit>, String> {
    runtime.session_history(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatsArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn session_stats(
    runtime: &RuntimeState,
    args: SessionStatsArgs,
) -> Result<SessionStats, String> {
    runtime.session_stats(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatsArgs {
    pub agent_name: String,
    pub range_days: Option<u32>,
}

/// Agent 级跨会话统计（趋势 / 模型 / 工具 / 成本估算）。
pub fn agent_stats(args: AgentStatsArgs) -> Result<crate::stats::AgentStats, String> {
    crate::stats::agent_stats(&args.agent_name, args.range_days)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionArgs {
    pub agent_name: String,
    pub session_id: String,
    pub up_to_entry_id: Option<String>,
}

pub fn fork_session(runtime: &RuntimeState, args: ForkSessionArgs) -> Result<SessionInfo, String> {
    runtime.fork_session(
        &args.agent_name,
        &args.session_id,
        args.up_to_entry_id.as_deref(),
    )
}

// ============ 归档 / 恢复 / 删除（Agent 与会话）============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveAgentArgs {
    pub name: String,
}

pub fn archive_agent(runtime: &RuntimeState, args: ArchiveAgentArgs) -> Result<(), String> {
    runtime.archive_agent(&args.name)?;
    // 归档后这个 Agent 不该再持有长驻的 MCP 子进程。
    runtime.shutdown_agent_mcp(&args.name);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreAgentArgs {
    pub name: String,
}

pub fn restore_agent(runtime: &RuntimeState, args: RestoreAgentArgs) -> Result<(), String> {
    let _ = runtime;
    agents::restore_agent(&args.name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAgentArgs {
    pub name: String,
    pub archived: Option<bool>,
}

pub fn delete_agent(runtime: &RuntimeState, args: DeleteAgentArgs) -> Result<(), String> {
    if args.archived.unwrap_or(false) {
        agents::delete_archived_agent(&args.name)
    } else {
        runtime.delete_agent(&args.name)?;
        runtime.shutdown_agent_mcp(&args.name);
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionsArgs {
    pub agent_name: String,
}

/// 把旧 JSONL 会话导入 git（逐条校验、幂等、原文件改名保留）。
///
/// 占用检查与脱离槽位在锁内同步做（快），读全部旧会话并 spawn git 的部分放到
/// 阻塞线程池执行（与技能安装同一条约定）。
pub async fn import_sessions(
    runtime: &RuntimeState,
    args: ImportSessionsArgs,
) -> Result<SessionImportReport, String> {
    runtime.prepare_session_import(&args.agent_name)?;
    let agent_name = args.agent_name.clone();
    tokio::task::spawn_blocking(move || {
        let def = agents::load_agent(&agent_name)?;
        let legacy_dir = def
            .sessions_dir()
            .ok_or("Cannot resolve session directory")?;
        Ok(pipi_session::import_legacy_sessions(
            &agent_repo(&def)?,
            &legacy_dir,
        ))
    })
    .await
    .map_err(|error| format!("Session import task failed: {error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSessionArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn archive_session(runtime: &RuntimeState, args: ArchiveSessionArgs) -> Result<(), String> {
    runtime.archive_session(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSessionArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn restore_session(runtime: &RuntimeState, args: RestoreSessionArgs) -> Result<(), String> {
    runtime.restore_session(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub agent_name: String,
    pub session_id: String,
    pub archived: Option<bool>,
}

pub fn delete_session(runtime: &RuntimeState, args: DeleteSessionArgs) -> Result<(), String> {
    if args.archived.unwrap_or(false) {
        runtime.delete_archived_session(&args.agent_name, &args.session_id)
    } else {
        runtime.delete_session(&args.agent_name, &args.session_id)
    }
}

// ============ 技能（av store 与 Agent 契约）============

/// 全局技能 store（`~/.av/skills`）一览：已安装版本、来源与内容存在性。
pub fn skill_library() -> Result<SkillLibrary, String> {
    pipi_venv::skills::store_skills()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillsArgs {
    pub agent_name: String,
}

/// 某个 Agent 的技能启用视图（`[resources.skills].use` + 解析状态）。
pub fn agent_skills(args: AgentSkillsArgs) -> Result<AgentSkills, String> {
    pipi_venv::skills::agent_skills(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSkillEnabledArgs {
    pub agent_name: String,
    pub name: String,
    pub enabled: bool,
}

/// 启用/禁用一个 store 技能（写契约 `use` 与同层 `agent.lock`）。
pub fn set_skill_enabled(args: SetSkillEnabledArgs) -> Result<AgentSkills, String> {
    pipi_venv::skills::set_skill_enabled(&args.agent_name, &args.name, args.enabled)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyStoreSkillsArgs {
    pub names: Option<Vec<String>>,
}

/// 重算 store 内容哈希。要读全部技能内容，放到阻塞线程池执行。
pub async fn verify_store_skills(args: VerifyStoreSkillsArgs) -> Result<Vec<SkillVerify>, String> {
    tokio::task::spawn_blocking(move || {
        pipi_venv::skills::verify_store_skills(args.names.as_deref())
    })
    .await
    .map_err(|error| format!("Skill verification task failed: {error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSkillArgs {
    pub source: String,
    pub skills: Option<Vec<String>>,
    pub all: bool,
    pub dry_run: bool,
}

/// 安装技能到全局 store（只装不启用）。git 源要联网，放到阻塞线程池执行。
pub async fn install_skill(args: InstallSkillArgs) -> Result<SkillInstallReport, String> {
    tokio::task::spawn_blocking(move || {
        pipi_venv::skills::install_skill(
            &args.source,
            args.skills.as_deref(),
            args.all,
            args.dry_run,
        )
    })
    .await
    .map_err(|error| format!("Skill installation task failed: {error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveStoreSkillArgs {
    pub name: String,
}

/// 从 store 移除一个技能的全部版本（仍被 Agent 声明时拒绝）。
pub fn remove_store_skill(args: RemoveStoreSkillArgs) -> Result<usize, String> {
    pipi_venv::skills::remove_store_skill(&args.name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAgentSkillsArgs {
    pub agent_name: String,
    pub repair: bool,
}

/// 同步/修复声明技能（可能联网取 git）。放到阻塞线程池执行。
pub async fn sync_agent_skills(args: SyncAgentSkillsArgs) -> Result<SkillSyncReport, String> {
    tokio::task::spawn_blocking(move || {
        pipi_venv::skills::sync_agent_skills(&args.agent_name, args.repair)
    })
    .await
    .map_err(|error| format!("Skill synchronization task failed: {error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUsageArgs {
    pub name: String,
}

/// 一个技能在各个 Agent 上的启用情况（技能库详情用）。
pub fn skill_usage(args: SkillUsageArgs) -> Result<Vec<SkillUsage>, String> {
    pipi_venv::skills::skill_usage(&args.name)
}

// ============ 论坛 ============

/// `forum_read` 的返回：主题元数据 + 本次读取的帖子。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumThread {
    pub thread: Thread,
    pub posts: Vec<Post>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumThreadsArgs {
    pub status: Option<String>,
    pub tag: Option<String>,
    pub archived: Option<bool>,
    pub limit: Option<usize>,
}

/// 列论坛主题（人在 UI / 命令里浏览）。
pub fn forum_threads(args: ForumThreadsArgs) -> Result<Vec<ThreadSummary>, String> {
    let repo = crate::forum::command_repo()?;
    let status = match args.status.as_deref() {
        None => None,
        Some("open") => Some(ThreadStatus::Open),
        Some("closed") => Some(ThreadStatus::Closed),
        Some(other) => return Err(format!("status must be open or closed (got {other:?})")),
    };
    let filter = ThreadFilter {
        status,
        tag: args.tag,
        include_archived: args.archived.unwrap_or(false),
    };
    repo.list_threads(&filter, args.limit.unwrap_or(20).clamp(1, 200))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumReadArgs {
    pub thread_id: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// 读一个主题（元数据 + 帖子，分页）。
pub fn forum_read(args: ForumReadArgs) -> Result<ForumThread, String> {
    let repo = crate::forum::command_repo()?;
    let thread = repo.thread(&args.thread_id)?;
    let posts = repo.posts(
        &args.thread_id,
        PostRange {
            offset: args.offset.unwrap_or(0),
            limit: args.limit,
        },
    )?;
    Ok(ForumThread { thread, posts })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumSearchArgs {
    pub query: String,
    pub archived: Option<bool>,
    pub limit: Option<usize>,
}

/// 检索论坛（标题 + 正文）。
pub fn forum_search(args: ForumSearchArgs) -> Result<Vec<SearchHit>, String> {
    let repo = crate::forum::command_repo()?;
    repo.search(
        &args.query,
        SearchOptions {
            limit: args.limit.unwrap_or(10).clamp(1, 100),
            include_archived: args.archived.unwrap_or(false),
        },
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumPostArgs {
    pub title: String,
    pub body: String,
    pub tags: Option<Vec<String>>,
    pub mentions: Option<Vec<String>>,
    pub author: Option<String>,
}

/// 人在 UI / 命令里发新主题（作者自报，`via = web`）。
pub fn forum_post(args: ForumPostArgs) -> Result<Thread, String> {
    let repo = crate::forum::command_repo()?;
    repo.create_thread(&NewThread {
        title: args.title,
        author: args.author.unwrap_or_else(|| "human".into()),
        body: args.body,
        tags: args.tags.unwrap_or_default(),
        mentions: args.mentions.unwrap_or_default(),
        via: PostVia::Web,
        origin: None,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumReplyArgs {
    pub thread_id: String,
    pub body: String,
    pub reply_to: Option<String>,
    pub mentions: Option<Vec<String>>,
    pub author: Option<String>,
}

/// 人在 UI / 命令里回帖。
pub fn forum_reply(args: ForumReplyArgs) -> Result<Post, String> {
    let repo = crate::forum::command_repo()?;
    repo.append_post(
        &args.thread_id,
        &NewPost {
            author: args.author.unwrap_or_else(|| "human".into()),
            body: args.body,
            via: PostVia::Web,
            reply_to: args.reply_to,
            mentions: args.mentions.unwrap_or_default(),
            origin: None,
        },
    )
}

// ============ Web / JSON transport 入口 ============

/// Web 宿主（以及任何 JSON transport）的唯一入口：按命令名解析参数并调用对应函数。
pub async fn dispatch(
    runtime: &RuntimeState,
    emitter: EventEmitter,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    match name {
        "list_agents" => to_value(list_agents(parse(args)?)?),
        "model_catalog" => to_value(model_catalog(parse(args)?).await?),
        "create_agent" => to_value(create_agent(parse(args)?)?),
        "save_agent" => to_value(save_agent(runtime, parse(args)?)?),
        "list_agent_files" => to_value(list_agent_files(parse(args)?)?),
        "read_agent_file" => to_value(read_agent_file(parse(args)?)?),
        "write_agent_file" => to_value(write_agent_file(runtime, parse(args)?)?),
        "read_notebook" => to_value(read_notebook(parse(args)?)?),
        "mcp_status" => to_value(mcp_status(runtime, parse(args)?)?),
        "list_workspace_files" => to_value(list_workspace_files(parse(args)?).await?),
        "preview_workspace_file" => to_value(preview_workspace_file(parse(args)?).await?),
        "workspace_changes" => to_value(workspace_changes(parse(args)?).await?),
        "workspace_diff" => to_value(workspace_diff(parse(args)?).await?),
        "list_icon_themes" => to_value(list_icon_themes().await?),
        "get_icon_theme" => to_value(get_icon_theme().await?),
        "set_icon_theme" => to_value(set_icon_theme(parse(args)?).await?),
        "get_settings" => to_value(get_settings()),
        "save_settings" => to_value(save_settings(parse(args)?)?),
        "web_push_public_key" => to_value(web_push_public_key(runtime).await?),
        "register_push_subscription" => {
            to_value(register_push_subscription(runtime, parse(args)?).await?)
        }
        "remove_push_subscription" => {
            to_value(remove_push_subscription(runtime, parse(args)?).await?)
        }
        "push_subscription_status" => {
            to_value(push_subscription_status(runtime, parse(args)?).await?)
        }
        "test_push_notification" => to_value(test_push_notification(runtime, parse(args)?).await?),
        "test_desktop_notification" => {
            to_value(test_desktop_notification(runtime, CommandSource::Web)?)
        }
        "remote_access_status" => {
            to_value(remote_access_status(runtime, CommandSource::Web).await?)
        }
        "configure_remote_access" => {
            to_value(configure_remote_access(runtime, CommandSource::Web, parse(args)?).await?)
        }
        "set_remote_access_enabled" => {
            to_value(set_remote_access_enabled(runtime, CommandSource::Web, parse(args)?).await?)
        }
        "remote_access_token" => {
            to_value(remote_access_token(runtime, CommandSource::Web, parse(args)?).await?)
        }
        "list_sessions" => to_value(list_sessions(runtime, parse(args)?)?),
        "open_session" => to_value(open_session(runtime, parse(args)?)?),
        "session_info" => to_value(session_info(runtime, parse(args)?)?),
        "session_infos" => to_value(session_infos(runtime)?),
        "answer_question" => to_value(answer_question(runtime, parse(args)?)?),
        "answer_approval" => to_value(answer_approval(runtime, parse(args)?)?),
        "open_workbench_session" => to_value(open_workbench_session(runtime, parse(args)?)?),
        "stop_run" => to_value(stop_run(runtime, parse(args)?)?),
        "query_background_tasks" => to_value(query_background_tasks(runtime, parse(args)?).await?),
        "manage_background_task" => to_value(manage_background_task(runtime, parse(args)?).await?),
        "new_session" => to_value(new_session(runtime, parse(args)?)?),
        "create_session" => to_value(create_session(runtime, parse(args)?)?),
        "browser_view" => to_value(browser_view(runtime, CommandSource::Web, parse(args)?).await?),
        "list_terminals" => to_value(list_terminals(runtime, parse(args)?)?),
        "open_terminal" => to_value(open_terminal(runtime, parse(args)?).await?),
        "read_terminal" => to_value(read_terminal(runtime, parse(args)?).await?),
        "write_terminal" => to_value(write_terminal(runtime, parse(args)?).await?),
        "resize_terminal" => to_value(resize_terminal(runtime, parse(args)?)?),
        "close_terminal" => to_value(close_terminal(runtime, parse(args)?)?),
        "send_prompt" => to_value(send_prompt(runtime, emitter, parse(args)?)?),
        "compact_now" => to_value(compact_now(runtime, emitter, parse(args)?)?),
        "steer" => to_value(steer(runtime, parse(args)?)?),
        "queue_prompt" => to_value(queue_prompt(runtime, parse(args)?)?),
        "cancel_queued_prompt" => to_value(cancel_queued_prompt(runtime, parse(args)?)?),
        "set_prompt_queue_paused" => to_value(set_prompt_queue_paused(runtime, parse(args)?)?),
        "set_session_model" => to_value(set_session_model(runtime, parse(args)?)?),
        "set_session_permission_mode" => {
            to_value(set_session_permission_mode(runtime, parse(args)?)?)
        }
        "session_messages" => to_value(session_messages(runtime, parse(args)?).await?),
        "session_history" => to_value(session_history(runtime, parse(args)?)?),
        "session_stats" => to_value(session_stats(runtime, parse(args)?)?),
        "agent_stats" => to_value(agent_stats(parse(args)?)?),
        "fork_session" => to_value(fork_session(runtime, parse(args)?)?),
        "archive_agent" => to_value(archive_agent(runtime, parse(args)?)?),
        "restore_agent" => to_value(restore_agent(runtime, parse(args)?)?),
        "delete_agent" => to_value(delete_agent(runtime, parse(args)?)?),
        "import_sessions" => to_value(import_sessions(runtime, parse(args)?).await?),
        "archive_session" => to_value(archive_session(runtime, parse(args)?)?),
        "restore_session" => to_value(restore_session(runtime, parse(args)?)?),
        "delete_session" => to_value(delete_session(runtime, parse(args)?)?),
        "skill_library" => to_value(skill_library()?),
        "agent_skills" => to_value(agent_skills(parse(args)?)?),
        "set_skill_enabled" => to_value(set_skill_enabled(parse(args)?)?),
        "verify_store_skills" => to_value(verify_store_skills(parse(args)?).await?),
        "install_skill" => to_value(install_skill(parse(args)?).await?),
        "remove_store_skill" => to_value(remove_store_skill(parse(args)?)?),
        "sync_agent_skills" => to_value(sync_agent_skills(parse(args)?).await?),
        "skill_usage" => to_value(skill_usage(parse(args)?)?),
        "forum_threads" => to_value(forum_threads(parse(args)?)?),
        "forum_read" => to_value(forum_read(parse(args)?)?),
        "forum_search" => to_value(forum_search(parse(args)?)?),
        "forum_post" => to_value(forum_post(parse(args)?)?),
        "forum_reply" => to_value(forum_reply(parse(args)?)?),
        other => Err(format!("Unknown command: {other}")),
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() {
        serde_json::json!({})
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| format!("Invalid command arguments: {error}"))
}

fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value)
        .map_err(|error| format!("Failed to serialize command result: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)] // HOME_LOCK 必须覆盖整个异步测试
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn commands_are_registered_once() {
        let unique: HashSet<&str> = COMMANDS.iter().copied().collect();
        assert_eq!(unique.len(), COMMANDS.len(), "COMMANDS 存在重复命令名");
        assert_eq!(
            COMMANDS.len(),
            80,
            "宿主命令数应为 80（前端 CommandMap 同步）"
        );
    }

    #[tokio::test]
    async fn remote_cannot_control_desktop_browser() {
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let emitter: EventEmitter = Arc::new(|_| {});
        let error = dispatch(
            &runtime,
            emitter,
            "browser_view",
            serde_json::json!({
                "agentName": "a", "sessionId": "s", "action": "state"
            }),
        )
        .await
        .unwrap_err();
        assert!(error.contains("only allowed in the desktop"));
    }

    #[tokio::test]
    async fn unknown_command_is_rejected() {
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let emitter: EventEmitter = Arc::new(|_| {});
        let error = dispatch(&runtime, emitter, "no_such_command", Value::Null)
            .await
            .expect_err("未知命令必须报错");
        assert!(
            error.contains("no_such_command"),
            "错误信息应包含命令名：{error}"
        );
    }

    #[tokio::test]
    async fn dispatch_parses_flat_camel_case_args() {
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let emitter: EventEmitter = Arc::new(|_| {});
        let value = dispatch(
            &runtime,
            emitter,
            "list_agents",
            serde_json::json!({ "archived": false }),
        )
        .await
        .expect("list_agents 应当成功");
        assert!(value.is_array());
    }
}
