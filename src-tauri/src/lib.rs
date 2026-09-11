mod chat;
mod commands;

pub fn run() {
    tauri::Builder::default()
        .manage(chat::ChatState::default())
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
