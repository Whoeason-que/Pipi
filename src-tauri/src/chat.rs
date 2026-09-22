//! Tauri 事件适配层。
//!
//! 会话运行逻辑位于 pipi-app::runtime，本模块只把共享运行时映射为
//! Tauri command 与 event。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use pipi_app::approval::ApprovalDecision;
use pipi_app::runtime::{self, EventEmitter, RuntimeEvent, SessionInfo};
use pipi_core::types::{Message, Model};

pub use pipi_app::runtime::RuntimeState as ChatState;

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
pub fn list_sessions(agent_name: String) -> Result<Vec<runtime::SessionSummary>, String> {
    runtime::list_sessions(&agent_name)
}

#[tauri::command]
pub fn open_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.open_session(&agent_name, &session_id)
}

#[tauri::command]
pub fn session_info(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<Option<SessionInfo>, String> {
    state.session_info(&agent_name, &session_id)
}

/// 所有打开中的会话：前端据此知道「哪些 Agent 正在跑」（多 Agent 并发下运行态是集合）。
#[tauri::command]
pub fn session_infos(state: State<ChatState>) -> Result<Vec<SessionInfo>, String> {
    state.session_infos()
}

/// 设置工作台的内存测试会话：每个 Agent 在应用运行期最多一条。
#[tauri::command]
pub fn ensure_test_session(
    state: State<ChatState>,
    agent_name: String,
) -> Result<SessionInfo, String> {
    state.ensure_test_session(&agent_name)
}

/// 清空临时测试上下文，并用磁盘上最新保存的 Agent 定义重建。
#[tauri::command]
pub fn reset_test_session(
    state: State<ChatState>,
    agent_name: String,
) -> Result<SessionInfo, String> {
    state.reset_test_session(&agent_name)
}

#[tauri::command]
pub fn session_running(state: State<ChatState>, agent_name: String, session_id: String) -> bool {
    state.session_running(&agent_name, &session_id)
}

#[tauri::command]
pub fn stop_run(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.stop_run(&agent_name, &session_id)
}

#[tauri::command]
pub async fn query_background_tasks(
    state: State<'_, ChatState>,
    agent_name: String,
    session_id: String,
    job_id: Option<String>,
    after_seq: Option<u64>,
    wait_ms: Option<u64>,
    include_completed: Option<bool>,
    limit: Option<usize>,
) -> Result<Vec<pipi_core::types::BackgroundTaskSnapshot>, String> {
    state
        .query_background_tasks(
            &agent_name,
            &session_id,
            job_id,
            after_seq.unwrap_or(0),
            wait_ms.unwrap_or(0),
            include_completed.unwrap_or(true),
            limit.unwrap_or(20),
        )
        .await
}

#[tauri::command]
pub async fn manage_background_task(
    state: State<'_, ChatState>,
    agent_name: String,
    session_id: String,
    job_id: String,
    action: String,
    data: Option<String>,
) -> Result<pipi_core::types::BackgroundTaskSnapshot, String> {
    state
        .manage_background_task(&agent_name, &session_id, &job_id, &action, data)
        .await
}

#[tauri::command]
pub fn new_session(state: State<ChatState>, agent_name: String) -> Result<(), String> {
    state.new_session(&agent_name)
}

#[tauri::command]
pub fn send_prompt(
    app: AppHandle,
    state: State<'_, ChatState>,
    agent_name: String,
    session_id: Option<String>,
    prompt: String,
    model: Option<Model>,
) -> Result<(), String> {
    state.send_prompt(
        &agent_name,
        session_id.as_deref(),
        &prompt,
        model,
        tauri_emitter(app),
    )
}

/// 手动压缩当前会话（跳过阈值预检，走与自动压缩相同的分叉/归档路径）。
#[tauri::command]
pub fn compact_now(
    app: AppHandle,
    state: State<'_, ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.compact_now(&agent_name, &session_id, tauri_emitter(app))
}

/// 运行中插话（steering）：注入当前运行的下一轮上下文。
#[tauri::command]
pub fn steer(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
    message: String,
) -> Result<(), String> {
    state.steer(&agent_name, &session_id, &message)
}

/// 回传 bash 命令审批请求的用户决定。
#[tauri::command]
pub fn resolve_approval(
    state: State<ChatState>,
    request_id: String,
    decision: String,
) -> Result<(), String> {
    state.resolve_approval(&request_id, decision.parse::<ApprovalDecision>()?)
}

#[tauri::command]
pub fn set_session_model(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
    model: Option<Model>,
) -> Result<(), String> {
    state.set_session_model(&agent_name, &session_id, model)
}

#[tauri::command]
pub async fn session_messages(
    state: State<'_, ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<Vec<Message>, String> {
    state.session_messages(&agent_name, &session_id).await
}

#[tauri::command]
pub fn session_stats(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<pipi_core::stats::SessionStats, String> {
    state.session_stats(&agent_name, &session_id)
}

#[tauri::command]
pub fn fork_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
    up_to_entry_id: Option<String>,
) -> Result<SessionInfo, String> {
    state.fork_session(&agent_name, &session_id, up_to_entry_id.as_deref())
}

// ============ 归档 / 恢复 / 删除（Agent 与会话） ============
// 归档 = 目录/文件移动到 `.archive/`（文件即真相，不引入新状态）；
// 删除不可恢复，UI 层负责确认。核心层负责运行中/后台任务的占用检查，
// 空闲的打开会话会在操作前自动释放。

#[tauri::command]
pub fn archive_agent(state: State<ChatState>, name: String) -> Result<(), String> {
    state.archive_agent(&name)
}

#[tauri::command]
pub fn restore_agent(state: State<ChatState>, name: String) -> Result<(), String> {
    state.restore_agent(&name)
}

#[tauri::command]
pub fn delete_agent(state: State<ChatState>, name: String) -> Result<(), String> {
    state.delete_agent(&name)
}

#[tauri::command]
pub fn delete_archived_agent(state: State<ChatState>, name: String) -> Result<(), String> {
    state.delete_archived_agent(&name)
}

#[tauri::command]
pub fn list_archived_sessions(
    state: State<ChatState>,
    agent_name: String,
) -> Result<Vec<runtime::SessionSummary>, String> {
    state.list_archived_sessions(&agent_name)
}

#[tauri::command]
pub fn archive_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.archive_session(&agent_name, &session_id)
}

#[tauri::command]
pub fn restore_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.restore_session(&agent_name, &session_id)
}

#[tauri::command]
pub fn delete_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.delete_session(&agent_name, &session_id)
}

#[tauri::command]
pub fn delete_archived_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    state.delete_archived_session(&agent_name, &session_id)
}
