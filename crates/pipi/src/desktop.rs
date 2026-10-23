//! 只装配桌面资源、事件和托盘；远程服务策略位于 pipi-core。
use std::sync::Arc;

use pipi_core::{
    remote::{HostLease, RemoteAccess},
    runtime::RuntimeState,
    server::{WebAsset, WebAssets},
};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
};

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(HostLease::acquire()?);
    let runtime = app.state::<Arc<RuntimeState>>().inner().clone();
    runtime
        .events()
        .set_desktop_sink(crate::chat::tauri_emitter(app.handle().clone()));
    let resolver = app.asset_resolver();
    let assets = WebAssets::Embedded(Arc::new(move |path| {
        resolver.get(path.into()).map(|asset| WebAsset {
            bytes: asset.bytes,
            content_type: asset.mime_type,
        })
    }));
    let remote = RemoteAccess::new(&runtime, assets);
    runtime.attach_remote_access(remote.clone())?;
    app.manage(remote.clone());

    let show = MenuItem::with_id(app, "show", "Show Pipi", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Pipi", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("pipi")
        .tooltip("Pipi · Remote access runtime can run in background")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    if let Err(error) = tray.build(app) {
        eprintln!("Failed to create system tray, closing window will exit: {error}");
    }
    tauri::async_runtime::spawn(async move {
        remote.restore().await;
    });
    Ok(())
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        let app = window.app_handle();
        if app.tray_by_id("pipi").is_some() && app.state::<Arc<RemoteAccess>>().keep_alive() {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}
