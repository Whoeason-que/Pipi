//! Tauri 事件适配层。
//!
//! 会话运行逻辑位于 pipi-core::runtime，本模块只把共享运行时映射为
//! Tauri command 与 event。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use pipi_core::runtime::{self, EventEmitter, RuntimeEvent, SessionInfo};
use pipi_core::types::Message;

pub use pipi_core::runtime::RuntimeState as ChatState;

fn tauri_emitter(app: AppHandle) -> EventEmitter {
    Arc::new(move |event| {
        let result = match event {
            RuntimeEvent::AgentEvent(payload) => app.emit("agent-event", &payload),
            RuntimeEvent::SessionStats(payload) => app.emit("session-stats", &payload),
            RuntimeEvent::SessionError(payload) => app.emit("session-error", &payload),
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
) -> Result<(), String> {
    state.send_prompt(&agent_name, &prompt, tauri_emitter(app))
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

