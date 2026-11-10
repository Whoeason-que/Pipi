#[doc(hidden)]
pub mod browser;
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

/// Only the application webview may dispatch Pipi commands. Remote child
/// webviews can navigate to the app's own origin, so origin checks alone are
/// insufficient. Plugin commands are governed separately by capabilities.
#[doc(hidden)]
pub fn restrict_invoke(
    invoke: tauri::ipc::Invoke<tauri::Wry>,
    handler: impl FnOnce(tauri::ipc::Invoke<tauri::Wry>) -> bool,
) -> bool {
    if invoke.message.webview_ref().label() != "main" {
        invoke.resolver.reject("Untrusted webview");
        return true;
    }
    handler(invoke)
}

/// 桌面壳的 command 跑在 GTK 主线程上，那里没有 reactor；把 Tauri 自己的
/// Tokio 运行时句柄交给核心，Agent 循环由 `RuntimeState` 显式 spawn 上去。
fn runtime_handle() -> tokio::runtime::Handle {
    tauri::async_runtime::handle().inner().clone()
}

pub fn run() {
    // 唤醒器需要 Arc<RuntimeState>（后台任务完成时由宿主推动会话继续），
    // 所以先建 Arc、启动订阅，再交给 Tauri 托管。
    let runtime_state = Arc::new(pipi_core::runtime::RuntimeState::new(runtime_handle()));
    runtime_state.start_background_waker();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(runtime_state)
        .setup(desktop::setup)
        .on_window_event(desktop::on_window_event)
        .invoke_handler(|invoke| {
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
                commands::model_catalog,
                commands::list_agents,
                commands::create_agent,
                commands::save_agent,
                commands::list_icon_themes,
                commands::get_icon_theme,
                commands::set_icon_theme,
                commands::get_settings,
                commands::save_settings,
                commands::web_push_public_key,
                commands::register_push_subscription,
                commands::remove_push_subscription,
                commands::push_subscription_status,
                commands::test_push_notification,
                commands::test_desktop_notification,
                commands::remote_access_status,
                commands::configure_remote_access,
                commands::set_remote_access_enabled,
                commands::remote_access_token,
                chat::send_prompt,
                chat::compact_now,
                chat::steer,
                chat::queue_prompt,
                chat::cancel_queued_prompt,
                chat::set_prompt_queue_paused,
                chat::stop_run,
                chat::query_background_tasks,
                chat::manage_background_task,
                chat::new_session,
                chat::create_session,
                chat::browser_view,
                chat::session_messages,
                chat::session_history,
                chat::session_stats,
                chat::agent_stats,
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
                commands::read_notebook,
                commands::mcp_status,
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
            ];
            restrict_invoke(invoke, handler)
        })
        .build(tauri::generate_context!())
        .expect("Failed to start Pipi");
    let saving = Arc::new(AtomicBool::new(false));
    let saved = Arc::new(AtomicBool::new(false));
    app.run(move |app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event
            && !saved.load(Ordering::SeqCst)
        {
            api.prevent_exit();
            if saving.swap(true, Ordering::SeqCst) {
                return;
            }
            let app = app.clone();
            let saving = saving.clone();
            let saved = saved.clone();
            let runtime = app
                .state::<Arc<pipi_core::runtime::RuntimeState>>()
                .inner()
                .clone();
            let remote = app
                .state::<Arc<pipi_core::remote::RemoteAccess>>()
                .inner()
                .clone();
            tauri::async_runtime::spawn(async move {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_title("Pipi — Stopping and saving…");
                }
                if let Err(error) = runtime.shutdown().await {
                    eprintln!("pipi: shutdown failed: {error}");
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.set_title(&format!(
                            "Pipi — Save failed; quit again to retry: {error}"
                        ));
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                    saving.store(false, Ordering::SeqCst);
                    return;
                }
                let _ = remote.set_enabled(false).await;
                saved.store(true, Ordering::SeqCst);
                app.exit(0);
            });
        }
    });
}
