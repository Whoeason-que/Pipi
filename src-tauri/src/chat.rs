//! Tauri 事件适配层。
//!
//! 会话运行逻辑位于 pipi-app::runtime，本模块只把共享运行时映射为
//! Tauri command 与 event；命令参数与转发目标见 `pipi_app::commands`。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use pipi_agent::stats::SessionStats;
use pipi_app::commands::{
    ArchiveAgentArgs, ArchiveSessionArgs, CompactNowArgs, DeleteAgentArgs, DeleteArchivedAgentArgs,
    DeleteArchivedSessionArgs, DeleteSessionArgs, EnsureTestSessionArgs, ForkSessionArgs,
    ListArchivedSessionsArgs, ListSessionsArgs, ManageBackgroundTaskArgs, NewSessionArgs,
    OpenSessionArgs, QueryBackgroundTasksArgs, ResetTestSessionArgs, ResolveApprovalArgs,
    RestoreAgentArgs, RestoreSessionArgs, SendPromptArgs, SessionInfoArgs, SessionMessagesArgs,
    SessionRunningArgs, SessionStatsArgs, SetSessionModelArgs, SteerArgs, StopRunArgs,
};
use pipi_app::events::{EventEmitter, RuntimeEvent};
use pipi_app::runtime::{RuntimeState, SessionInfo};
use pipi_protocol::session::SessionSummary;
use pipi_protocol::{BackgroundTaskSnapshot, Message};

use crate::invoke_args::InvokeArgs;

fn tauri_emitter(app: AppHandle) -> EventEmitter {
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
pub fn list_sessions(args: InvokeArgs<ListSessionsArgs>) -> Result<Vec<SessionSummary>, String> {
    pipi_app::commands::list_sessions(args.0)
}

#[tauri::command]
pub fn open_session(
    state: State<RuntimeState>,
    args: InvokeArgs<OpenSessionArgs>,
) -> Result<(), String> {
    pipi_app::commands::open_session(&state, args.0)
}

#[tauri::command]
pub fn session_info(
    state: State<RuntimeState>,
    args: InvokeArgs<SessionInfoArgs>,
) -> Result<Option<SessionInfo>, String> {
    pipi_app::commands::session_info(&state, args.0)
}

/// 所有打开中的会话：前端据此知道「哪些 Agent 正在跑」（多 Agent 并发下运行态是集合）。
#[tauri::command]
pub fn session_infos(state: State<RuntimeState>) -> Result<Vec<SessionInfo>, String> {
    pipi_app::commands::session_infos(&state)
}

/// 设置工作台的内存测试会话：每个 Agent 在应用运行期最多一条。
#[tauri::command]
pub fn ensure_test_session(
    state: State<RuntimeState>,
    args: InvokeArgs<EnsureTestSessionArgs>,
) -> Result<SessionInfo, String> {
    pipi_app::commands::ensure_test_session(&state, args.0)
}

/// 清空临时测试上下文，并用磁盘上最新保存的 Agent 定义重建。
#[tauri::command]
pub fn reset_test_session(
    state: State<RuntimeState>,
    args: InvokeArgs<ResetTestSessionArgs>,
) -> Result<SessionInfo, String> {
    pipi_app::commands::reset_test_session(&state, args.0)
}

#[tauri::command]
pub fn session_running(state: State<RuntimeState>, args: InvokeArgs<SessionRunningArgs>) -> bool {
    pipi_app::commands::session_running(&state, args.0)
}

#[tauri::command]
pub fn stop_run(state: State<RuntimeState>, args: InvokeArgs<StopRunArgs>) -> Result<(), String> {
    pipi_app::commands::stop_run(&state, args.0)
}

#[tauri::command]
pub async fn query_background_tasks(
    state: State<'_, RuntimeState>,
    args: InvokeArgs<QueryBackgroundTasksArgs>,
) -> Result<Vec<BackgroundTaskSnapshot>, String> {
    pipi_app::commands::query_background_tasks(&state, args.0).await
}

#[tauri::command]
pub async fn manage_background_task(
    state: State<'_, RuntimeState>,
    args: InvokeArgs<ManageBackgroundTaskArgs>,
) -> Result<BackgroundTaskSnapshot, String> {
    pipi_app::commands::manage_background_task(&state, args.0).await
}

#[tauri::command]
pub fn new_session(
    state: State<RuntimeState>,
    args: InvokeArgs<NewSessionArgs>,
) -> Result<(), String> {
    pipi_app::commands::new_session(&state, args.0)
}

#[tauri::command]
pub fn send_prompt(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    args: InvokeArgs<SendPromptArgs>,
) -> Result<(), String> {
    pipi_app::commands::send_prompt(&state, tauri_emitter(app), args.0)
}

/// 手动压缩当前会话（跳过阈值预检，走与自动压缩相同的分叉/归档路径）。
#[tauri::command]
pub fn compact_now(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    args: InvokeArgs<CompactNowArgs>,
) -> Result<(), String> {
    pipi_app::commands::compact_now(&state, tauri_emitter(app), args.0)
}

/// 运行中插话（steering）：注入当前运行的下一轮上下文。
#[tauri::command]
pub fn steer(state: State<RuntimeState>, args: InvokeArgs<SteerArgs>) -> Result<(), String> {
    pipi_app::commands::steer(&state, args.0)
}

/// 回传 bash 命令审批请求的用户决定。
#[tauri::command]
pub fn resolve_approval(
    state: State<RuntimeState>,
    args: InvokeArgs<ResolveApprovalArgs>,
) -> Result<(), String> {
    pipi_app::commands::resolve_approval(&state, args.0)
}

#[tauri::command]
pub fn set_session_model(
    state: State<RuntimeState>,
    args: InvokeArgs<SetSessionModelArgs>,
) -> Result<(), String> {
    pipi_app::commands::set_session_model(&state, args.0)
}

#[tauri::command]
pub async fn session_messages(
    state: State<'_, RuntimeState>,
    args: InvokeArgs<SessionMessagesArgs>,
) -> Result<Vec<Message>, String> {
    pipi_app::commands::session_messages(&state, args.0).await
}

#[tauri::command]
pub fn session_stats(
    state: State<RuntimeState>,
    args: InvokeArgs<SessionStatsArgs>,
) -> Result<SessionStats, String> {
    pipi_app::commands::session_stats(&state, args.0)
}

#[tauri::command]
pub fn fork_session(
    state: State<RuntimeState>,
    args: InvokeArgs<ForkSessionArgs>,
) -> Result<SessionInfo, String> {
    pipi_app::commands::fork_session(&state, args.0)
}

// ============ 归档 / 恢复 / 删除（Agent 与会话） ============
// 归档 = 目录/文件移动到 `.archive/`（文件即真相，不引入新状态）；
// 删除不可恢复，UI 层负责确认。核心层负责运行中/后台任务的占用检查，
// 空闲的打开会话会在操作前自动释放。

#[tauri::command]
pub fn archive_agent(
    state: State<RuntimeState>,
    args: InvokeArgs<ArchiveAgentArgs>,
) -> Result<(), String> {
    pipi_app::commands::archive_agent(&state, args.0)
}

#[tauri::command]
pub fn restore_agent(
    state: State<RuntimeState>,
    args: InvokeArgs<RestoreAgentArgs>,
) -> Result<(), String> {
    pipi_app::commands::restore_agent(&state, args.0)
}

#[tauri::command]
pub fn delete_agent(
    state: State<RuntimeState>,
    args: InvokeArgs<DeleteAgentArgs>,
) -> Result<(), String> {
    pipi_app::commands::delete_agent(&state, args.0)
}

#[tauri::command]
pub fn delete_archived_agent(
    state: State<RuntimeState>,
    args: InvokeArgs<DeleteArchivedAgentArgs>,
) -> Result<(), String> {
    pipi_app::commands::delete_archived_agent(&state, args.0)
}

#[tauri::command]
pub fn list_archived_sessions(
    state: State<RuntimeState>,
    args: InvokeArgs<ListArchivedSessionsArgs>,
) -> Result<Vec<SessionSummary>, String> {
    pipi_app::commands::list_archived_sessions(&state, args.0)
}

#[tauri::command]
pub fn archive_session(
    state: State<RuntimeState>,
    args: InvokeArgs<ArchiveSessionArgs>,
) -> Result<(), String> {
    pipi_app::commands::archive_session(&state, args.0)
}

#[tauri::command]
pub fn restore_session(
    state: State<RuntimeState>,
    args: InvokeArgs<RestoreSessionArgs>,
) -> Result<(), String> {
    pipi_app::commands::restore_session(&state, args.0)
}

#[tauri::command]
pub fn delete_session(
    state: State<RuntimeState>,
    args: InvokeArgs<DeleteSessionArgs>,
) -> Result<(), String> {
    pipi_app::commands::delete_session(&state, args.0)
}

#[tauri::command]
pub fn delete_archived_session(
    state: State<RuntimeState>,
    args: InvokeArgs<DeleteArchivedSessionArgs>,
) -> Result<(), String> {
    pipi_app::commands::delete_archived_session(&state, args.0)
}
