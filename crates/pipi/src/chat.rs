//! Tauri 事件适配层。
//!
//! 会话运行逻辑位于 `pipi_core::runtime`，本模块只把共享运行时映射为
//! Tauri command 与 event；命令参数与转发目标见 `pipi_core::commands`。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use pipi_agent::stats::SessionStats;
use pipi_core::commands::{
    ArchiveAgentArgs, ArchiveSessionArgs, CompactNowArgs, CreateSessionArgs, DeleteAgentArgs,
    DeleteSessionArgs, ForkSessionArgs, ImportSessionsArgs, ListSessionsArgs,
    ManageBackgroundTaskArgs, NewSessionArgs, OpenSessionArgs, OpenWorkbenchSessionArgs,
    QueryBackgroundTasksArgs, ResolveApprovalArgs, RestoreAgentArgs, RestoreSessionArgs,
    SendPromptArgs, SessionInfoArgs, SessionMessagesArgs, SessionStatsArgs, SetSessionModelArgs,
    SteerArgs, StopRunArgs,
};
use pipi_core::events::{EventEmitter, RuntimeEvent};
use pipi_core::runtime::{RuntimeState, SessionInfo, SessionSnapshot};
use pipi_protocol::session::SessionSummary;
use pipi_protocol::{BackgroundTaskSnapshot, Message};

use crate::invoke_args::InvokeArgs;

pub(crate) fn tauri_emitter(app: AppHandle) -> EventEmitter {
    Arc::new(move |event| {
        let result = match event {
            RuntimeEvent::AgentEvent(payload) => app.emit("agent-event", &payload),
            RuntimeEvent::SessionStats(payload) => app.emit("session-stats", &payload),
            RuntimeEvent::SessionError(payload) => app.emit("session-error", &payload),
            RuntimeEvent::ApprovalRequest(payload) => app.emit("approval-request", &payload),
            RuntimeEvent::SessionSwitched(payload) => app.emit("session-switched", &payload),
            RuntimeEvent::SessionChanged(payload) => app.emit("session-changed", &payload),
        };
        let _ = result;
    })
}

#[tauri::command]
pub fn list_sessions(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<ListSessionsArgs>,
) -> Result<Vec<SessionSummary>, String> {
    pipi_core::commands::list_sessions(&state, args.0)
}

#[tauri::command]
pub fn open_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<OpenSessionArgs>,
) -> Result<SessionSnapshot, String> {
    pipi_core::commands::open_session(&state, args.0)
}

#[tauri::command]
pub fn session_info(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<SessionInfoArgs>,
) -> Result<Option<SessionInfo>, String> {
    pipi_core::commands::session_info(&state, args.0)
}

/// 所有打开中的会话：前端据此知道「哪些 Agent 正在跑」（多 Agent 并发下运行态是集合）。
#[tauri::command]
pub fn session_infos(state: State<Arc<RuntimeState>>) -> Result<Vec<SessionInfo>, String> {
    pipi_core::commands::session_infos(&state)
}

/// 打开或重置工作台测试会话。
#[tauri::command]
pub fn open_workbench_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<OpenWorkbenchSessionArgs>,
) -> Result<SessionInfo, String> {
    pipi_core::commands::open_workbench_session(&state, args.0)
}

#[tauri::command]
pub fn stop_run(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<StopRunArgs>,
) -> Result<(), String> {
    pipi_core::commands::stop_run(&state, args.0)
}

#[tauri::command]
pub async fn query_background_tasks(
    state: State<'_, Arc<RuntimeState>>,
    args: InvokeArgs<QueryBackgroundTasksArgs>,
) -> Result<Vec<BackgroundTaskSnapshot>, String> {
    pipi_core::commands::query_background_tasks(&state, args.0).await
}

#[tauri::command]
pub async fn manage_background_task(
    state: State<'_, Arc<RuntimeState>>,
    args: InvokeArgs<ManageBackgroundTaskArgs>,
) -> Result<BackgroundTaskSnapshot, String> {
    pipi_core::commands::manage_background_task(&state, args.0).await
}

#[tauri::command]
pub fn new_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<NewSessionArgs>,
) -> Result<(), String> {
    pipi_core::commands::new_session(&state, args.0)
}

#[tauri::command]
pub fn create_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<CreateSessionArgs>,
) -> Result<SessionInfo, String> {
    pipi_core::commands::create_session(&state, args.0)
}

#[tauri::command]
pub fn send_prompt(
    state: State<'_, Arc<RuntimeState>>,
    args: InvokeArgs<SendPromptArgs>,
) -> Result<SessionInfo, String> {
    pipi_core::commands::send_prompt(&state, state.events().emitter(), args.0)
}

/// 手动压缩当前会话（跳过阈值预检，走与自动压缩相同的分叉/归档路径）。
#[tauri::command]
pub fn compact_now(
    state: State<'_, Arc<RuntimeState>>,
    args: InvokeArgs<CompactNowArgs>,
) -> Result<(), String> {
    pipi_core::commands::compact_now(&state, state.events().emitter(), args.0)
}

/// 运行中插话（steering）：注入当前运行的下一轮上下文。
#[tauri::command]
pub fn steer(state: State<Arc<RuntimeState>>, args: InvokeArgs<SteerArgs>) -> Result<(), String> {
    pipi_core::commands::steer(&state, args.0)
}

/// 回传 bash 命令审批请求的用户决定。
#[tauri::command]
pub fn resolve_approval(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<ResolveApprovalArgs>,
) -> Result<(), String> {
    pipi_core::commands::resolve_approval(&state, args.0)
}

#[tauri::command]
pub fn set_session_model(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<SetSessionModelArgs>,
) -> Result<(), String> {
    pipi_core::commands::set_session_model(&state, args.0)
}

#[tauri::command]
pub async fn session_messages(
    state: State<'_, Arc<RuntimeState>>,
    args: InvokeArgs<SessionMessagesArgs>,
) -> Result<Vec<Message>, String> {
    pipi_core::commands::session_messages(&state, args.0).await
}

#[tauri::command]
pub fn session_stats(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<SessionStatsArgs>,
) -> Result<SessionStats, String> {
    pipi_core::commands::session_stats(&state, args.0)
}

#[tauri::command]
pub fn fork_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<ForkSessionArgs>,
) -> Result<SessionInfo, String> {
    pipi_core::commands::fork_session(&state, args.0)
}

// ============ 归档 / 恢复 / 删除（Agent 与会话） ============
// 归档 = 目录/文件移动到 `.archive/`（文件即真相，不引入新状态）；
// 删除不可恢复，UI 层负责确认。核心层负责运行中/后台任务的占用检查，
// 空闲的打开会话会在操作前自动释放。

#[tauri::command]
pub fn archive_agent(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<ArchiveAgentArgs>,
) -> Result<(), String> {
    pipi_core::commands::archive_agent(&state, args.0)
}

#[tauri::command]
pub fn restore_agent(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<RestoreAgentArgs>,
) -> Result<(), String> {
    pipi_core::commands::restore_agent(&state, args.0)
}

#[tauri::command]
pub fn delete_agent(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<DeleteAgentArgs>,
) -> Result<(), String> {
    pipi_core::commands::delete_agent(&state, args.0)
}

/// 把旧 JSONL 会话导入 git（逐条校验、幂等、原文件改名保留）。
#[tauri::command]
pub async fn import_sessions(
    state: State<'_, Arc<RuntimeState>>,
    args: InvokeArgs<ImportSessionsArgs>,
) -> Result<pipi_protocol::session::SessionImportReport, String> {
    pipi_core::commands::import_sessions(&state, args.0).await
}

#[tauri::command]
pub fn archive_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<ArchiveSessionArgs>,
) -> Result<(), String> {
    pipi_core::commands::archive_session(&state, args.0)
}

#[tauri::command]
pub fn restore_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<RestoreSessionArgs>,
) -> Result<(), String> {
    pipi_core::commands::restore_session(&state, args.0)
}

#[tauri::command]
pub fn delete_session(
    state: State<Arc<RuntimeState>>,
    args: InvokeArgs<DeleteSessionArgs>,
) -> Result<(), String> {
    pipi_core::commands::delete_session(&state, args.0)
}
