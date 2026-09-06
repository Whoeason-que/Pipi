mod commands;

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::list_agents,
            commands::create_agent,
            commands::load_agent,
            commands::save_agent,
        ])
        .run(tauri::generate_context!())
        .expect("Pipi 启动失败");
}
