//! Tauri 事件适配层。
//!
//! 会话运行逻辑位于 pipi-core::runtime，本模块只把共享运行时映射为
//! Tauri command 与 event。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use pipi_core::approval::ApprovalDecision;
use pipi_core::runtime::{self, EventEmitter, RuntimeEvent, SessionInfo};
use pipi_core::types::{Message, Model};

pub use pipi_core::runtime::RuntimeState as ChatState;

fn tauri_emitter(app: AppHandle) -> EventEmitter {
    Arc::new(move |event| {
        let result = match event {
            RuntimeEvent::AgentEvent(payload) => app.emit("agent-event", &payload),
            RuntimeEvent::SessionStats(payload) => app.emit("session-stats", &payload),
            RuntimeEvent::SessionError(payload) => app.emit("session-error", &payload),
            RuntimeEvent::ApprovalRequest(payload) => app.emit("approval-request", &payload),
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
pub fn session_info(state: State<ChatState>) -> Result<Option<SessionInfo>, String> {
    state.session_info()
}

#[tauri::command]
pub fn session_running(state: State<ChatState>) -> bool {
    state.session_running()
}

#[tauri::command]
pub fn stop_run(state: State<ChatState>) -> Result<(), String> {
    state.stop_run()
}

#[tauri::command]
pub fn new_session(state: State<ChatState>) -> Result<(), String> {
    state.new_session()
}

#[tauri::command]
pub fn send_prompt(
    app: AppHandle,
    state: State<'_, ChatState>,
    agent_name: String,
    prompt: String,
    model: Option<Model>,
) -> Result<(), String> {
    state.send_prompt(&agent_name, &prompt, model, tauri_emitter(app))
}

/// 运行中插话（steering）：注入当前运行的下一轮上下文。
#[tauri::command]
pub fn steer(state: State<ChatState>, message: String) -> Result<(), String> {
    state.steer(&message)
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
pub fn set_session_model(state: State<ChatState>, model: Option<Model>) -> Result<(), String> {
    state.set_session_model(model)
}

#[tauri::command]
pub async fn session_messages(state: State<'_, ChatState>) -> Result<Vec<Message>, String> {
    state.session_messages().await
}

#[tauri::command]
pub fn session_stats(state: State<ChatState>) -> Result<pipi_core::stats::SessionStats, String> {
    state.session_stats()
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
// 删除不可恢复，UI 层负责确认。核心层负责「会话打开中禁止操作」的占用检查。

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

