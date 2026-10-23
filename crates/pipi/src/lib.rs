mod chat;
mod desktop;

use pipi_core::remote::RemoteAccessControl;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::Manager;
mod commands;
mod invoke_args;

/// 桌面壳的 command 跑在 GTK 主线程上，那里没有 reactor；把 Tauri 自己的
/// Tokio 运行时句柄交给核心，Agent 循环由 `RuntimeState` 显式 spawn 上去。
fn runtime_handle() -> tokio::runtime::Handle {
    tauri::async_runtime::handle().inner().clone()
}

pub fn run() {
    let app = tauri::Builder::default()
        .manage(Arc::new(pipi_core::runtime::RuntimeState::new(
            runtime_handle(),
        )))
        .setup(desktop::setup)
        .on_window_event(desktop::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::model_catalog,
            commands::list_agents,
            commands::create_agent,
            commands::save_agent,
            commands::get_settings,
            commands::save_settings,
            commands::remote_access_status,
            commands::configure_remote_access,
            commands::set_remote_access_enabled,
            commands::remote_access_token,
            chat::send_prompt,
            chat::compact_now,
            chat::steer,
            chat::resolve_approval,
            chat::stop_run,
            chat::query_background_tasks,
            chat::manage_background_task,
            chat::new_session,
            chat::session_messages,
            chat::session_stats,
            chat::list_sessions,
            chat::open_session,
            chat::session_info,
            chat::session_infos,
            chat::open_workbench_session,
            chat::fork_session,
            chat::set_session_model,
            chat::archive_agent,
            chat::restore_agent,
            chat::delete_agent,
            chat::archive_session,
            chat::restore_session,
            chat::delete_session,
            chat::import_sessions,
            commands::list_agent_files,
            commands::read_agent_file,
            commands::write_agent_file,
            commands::list_workspace_files,
            commands::preview_workspace_file,
            commands::workspace_changes,
            commands::workspace_diff,
            commands::skill_library,
            commands::agent_skills,
            commands::set_skill_enabled,
            commands::verify_store_skills,
            commands::install_skill,
            commands::remove_store_skill,
            commands::sync_agent_skills,
            commands::skill_usage,
            commands::forum_threads,
            commands::forum_read,
            commands::forum_search,
            commands::forum_post,
            commands::forum_reply,
        ])
        .build(tauri::generate_context!())
        .expect("Failed to start Pipi");
    let exiting = Arc::new(AtomicBool::new(false));
    app.run(move |app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event
            && !exiting.swap(true, Ordering::SeqCst)
        {
            api.prevent_exit();
            let app = app.clone();
            let remote = app
                .state::<Arc<pipi_core::remote::RemoteAccess>>()
                .inner()
                .clone();
            tauri::async_runtime::spawn(async move {
                let _ = remote.set_enabled(false).await;
                app.exit(0);
            });
        }
    });
}
