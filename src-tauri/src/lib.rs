mod chat;
mod commands;

/// 桌面壳的 command 跑在 GTK 主线程上，那里没有 reactor；把 Tauri 自己的
/// Tokio 运行时句柄交给核心，Agent 循环由 `RuntimeState` 显式 spawn 上去。
fn runtime_handle() -> tokio::runtime::Handle {
    tauri::async_runtime::handle().inner().clone()
}

pub fn run() {
    tauri::Builder::default()
        .manage(chat::ChatState::new(runtime_handle()))
        .invoke_handler(tauri::generate_handler![
            commands::list_agents,
            commands::create_agent,
            commands::load_agent,
            commands::save_agent,
            commands::get_settings,
            commands::save_settings,
            chat::send_prompt,
            chat::stop_run,
            chat::new_session,
            chat::session_messages,
            chat::session_running,
            chat::session_stats,
            chat::list_sessions,
            chat::open_session,
            chat::session_info,
            chat::fork_session,
            chat::set_session_model,
        ])
        .run(tauri::generate_context!())
        .expect("Pipi 启动失败");
}
