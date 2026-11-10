//! Platform-specific browser operations.
#[cfg(target_os = "linux")]
use super::browser_url;
use super::{BrowserBounds, BrowserResult, BrowserViewState};
#[cfg(target_os = "linux")]
use std::sync::Mutex;

#[cfg(target_os = "linux")]
pub(super) async fn layout(
    view: &tauri::Webview,
    bounds: BrowserBounds,
) -> Result<BrowserViewState, String> {
    use gtk::prelude::*;
    if bounds.width == 0
        || bounds.height == 0
        || bounds.width > 16384
        || bounds.height > 16384
        || bounds.x.abs_diff(0) > 32768
        || bounds.y.abs_diff(0) > 32768
    {
        return Err("Invalid browser bounds.".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |native| {
        let child = native.inner();
        let result = (|| -> Result<(), String> {
            let parent = child.parent().ok_or("Browser has no native parent.")?;
            let fixed = if let Ok(fixed) = parent.clone().downcast::<gtk::Fixed>() {
                fixed
            } else {
                let container = parent
                    .downcast::<gtk::Box>()
                    .map_err(|_| "Unsupported browser container.")?;
                let overlay = container.children().into_iter().find_map(|widget| {
                    widget
                        .downcast::<gtk::Overlay>()
                        .ok()
                        .filter(|widget| widget.widget_name() == "pipi-browser-overlay")
                });
                let overlay = if let Some(overlay) = overlay {
                    overlay
                } else {
                    let main = container
                        .children()
                        .into_iter()
                        .find(|widget| {
                            widget.is::<webkit2gtk::WebView>()
                                && widget != child.upcast_ref::<gtk::Widget>()
                        })
                        .ok_or("Main application view not found.")?;
                    let overlay = gtk::Overlay::new();
                    overlay.set_widget_name("pipi-browser-overlay");
                    let fixed = gtk::Fixed::new();
                    fixed.set_widget_name("pipi-browser-layer");
                    fixed.set_hexpand(true);
                    fixed.set_vexpand(true);
                    container.remove(&main);
                    overlay.add(&main);
                    overlay.add_overlay(&fixed);
                    container.pack_start(&overlay, true, true, 0);
                    overlay.show_all();
                    overlay
                };
                let fixed = overlay
                    .children()
                    .into_iter()
                    .find_map(|widget| {
                        widget
                            .downcast::<gtk::Fixed>()
                            .ok()
                            .filter(|widget| widget.widget_name() == "pipi-browser-layer")
                    })
                    .ok_or("Browser layer not found.")?;
                container.remove(&child);
                fixed.put(&child, bounds.x, bounds.y);
                fixed
            };
            // The fixed layer covers the entire application window.  Its own
            // input window must let clicks outside the child WebView reach the
            // main UI; the WebView retains its own input window above it.
            if let Some(overlay) = fixed
                .parent()
                .and_then(|parent| parent.downcast::<gtk::Overlay>().ok())
            {
                overlay.set_overlay_pass_through(&fixed, true);
            }
            child.set_size_request(bounds.width as i32, bounds.height as i32);
            fixed.move_(&child, bounds.x, bounds.y);
            Ok(())
        })();
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "Browser view closed during layout.".to_string())??;
    browser_ui(view, "state".into()).await
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn layout(
    _view: &tauri::Webview,
    _bounds: BrowserBounds,
) -> Result<BrowserViewState, String> {
    Err("Native browser layout is not required on this platform.".into())
}

#[cfg(target_os = "linux")]
pub(super) async fn browser_ui(
    view: &tauri::Webview,
    action: String,
) -> Result<BrowserViewState, String> {
    use webkit2gtk::WebViewExt;
    let label = view.label().to_owned();
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |native| {
        let view = native.inner();
        match action.as_str() {
            "back" if view.can_go_back() => view.go_back(),
            "forward" if view.can_go_forward() => view.go_forward(),
            "reload" => view.reload(),
            _ => {}
        }
        let state = BrowserViewState {
            label,
            url: view.uri().map(|s| s.to_string()).unwrap_or_default(),
            title: view.title().map(|s| s.to_string()).unwrap_or_default(),
            loading: view.is_loading(),
            can_go_back: view.can_go_back(),
            can_go_forward: view.can_go_forward(),
        };
        let _ = tx.send(state);
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Browser view closed.".to_string())
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn browser_ui(
    _view: &tauri::Webview,
    _action: String,
) -> Result<BrowserViewState, String> {
    Err("Browser controls are currently available on Linux only.".into())
}

#[cfg(target_os = "linux")]
pub(super) async fn navigate(view: &tauri::Webview, url: tauri::Url) -> Result<String, String> {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        time::Duration,
    };
    use webkit2gtk::{LoadEvent, WebViewExt, glib, glib::prelude::ObjectExt};
    struct Navigation {
        started: Cell<bool>,
        committed: Cell<bool>,
        sender: RefCell<Option<tokio::sync::oneshot::Sender<Result<String, String>>>>,
        handlers: RefCell<Vec<glib::SignalHandlerId>>,
    }
    fn finish(view: &webkit2gtk::WebView, state: &Navigation, result: Result<String, String>) {
        if let Some(sender) = state.sender.borrow_mut().take() {
            let _ = sender.send(result);
            for id in state.handlers.borrow_mut().drain(..) {
                view.disconnect(id);
            }
        }
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |native| {
        let view = native.inner();
        let state = Rc::new(Navigation {
            started: Cell::new(false),
            committed: Cell::new(false),
            sender: RefCell::new(Some(tx)),
            handlers: RefCell::new(Vec::new()),
        });
        let current = state.clone();
        let changed = view.connect_load_changed(move |view, event| match event {
            LoadEvent::Started => current.started.set(true),
            LoadEvent::Committed if current.started.get() => current.committed.set(true),
            LoadEvent::Finished if current.started.get() => finish(
                view,
                &current,
                match view.uri().map(|s| s.to_string()) {
                    Some(url) if current.committed.get() && browser_url(&url).is_ok() => Ok(format!("Page loaded: {url}")),
                    _ => Err("Navigation did not commit an HTTP or HTTPS page; the page state may have changed.".into()),
                },
            ),
            _ => {}
        });
        let current = state.clone();
        let failed = view.connect_load_failed(move |view, _, _, error| {
            if current.started.get() {
                finish(
                    view,
                    &current,
                    Err(format!(
                        "Navigation failed: {error}; the page state may have changed."
                    )),
                );
            }
            false
        });
        state.handlers.borrow_mut().extend([changed, failed]);
        let weak = view.downgrade();
        glib::timeout_add_local_once(Duration::from_secs(15), move || {
            if let Some(view) = weak.upgrade() {
                finish(
                    &view,
                    &state,
                    Err("Navigation timed out; the page may still change.".into()),
                );
            }
        });
        view.load_uri(url.as_str());
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "Browser view closed during navigation.".to_string())?
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn navigate(_view: &tauri::Webview, _url: tauri::Url) -> Result<String, String> {
    Err("Agent browser navigation is currently available on Linux only.".into())
}

#[cfg(target_os = "linux")]
#[allow(deprecated)] // WebKitGTK 2.22 supports isolated worlds on older Linux installations.
pub(super) async fn evaluate_isolated(
    view: &tauri::Webview,
    script: String,
) -> Result<String, String> {
    use std::collections::HashSet;
    use webkit2gtk::{
        UserContentInjectedFrames, UserContentManagerExt, UserScript, UserScriptInjectionTime,
        WebViewExt,
    };
    static WORLDS: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    let label = view.label().to_owned();
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |native| {
        // Register a persistent named world: without a user script WebKitGTK may
        // discard the world and its element references between evaluations.
        let mut worlds = WORLDS
            .get_or_init(Default::default)
            .lock()
            .expect("browser worlds lock");
        if worlds.insert(label) {
            if let Some(manager) = native.inner().user_content_manager() {
                manager.add_script(&UserScript::for_world(
                    "void 0",
                    UserContentInjectedFrames::TopFrame,
                    UserScriptInjectionTime::Start,
                    "pipi-browser-control",
                    &[],
                    &[],
                ));
            }
        }
        drop(worlds);
        native.inner().run_javascript_in_world(
            &script,
            "pipi-browser-control",
            None::<&webkit2gtk::gio::Cancellable>,
            move |result| {
                let value = result.map_err(|e| e.to_string()).and_then(|result| {
                    result
                        .js_value()
                        .map(|v| v.to_string())
                        .ok_or_else(|| "Browser returned no result.".to_string())
                });
                let _ = tx.send(value);
            },
        );
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "Browser view closed during operation.".to_string())?
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn evaluate_isolated(
    _view: &tauri::Webview,
    _script: String,
) -> Result<String, String> {
    Err("Browser inspection and interaction require an isolated script environment; this platform is not supported yet.".into())
}

#[cfg(target_os = "linux")]
pub(super) async fn screenshot(view: &tauri::Webview) -> Result<BrowserResult, String> {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
    struct LimitedPng(Vec<u8>);
    impl std::io::Write for LimitedPng {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > (5 * 1024 * 1024usize).saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("Browser screenshot exceeds 5 MiB."));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |native| {
        native.inner().snapshot(
            SnapshotRegion::Visible,
            SnapshotOptions::empty(),
            None::<&webkit2gtk::gio::Cancellable>,
            move |result| {
                let result = result.map_err(|e| e.to_string()).and_then(|surface| {
                    let mut bytes = LimitedPng(Vec::new());
                    surface
                        .write_to_png(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    Ok(BrowserResult::Png(bytes.0))
                });
                let _ = tx.send(result);
            },
        );
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "Browser view closed during screenshot.".to_string())?
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn screenshot(_view: &tauri::Webview) -> Result<BrowserResult, String> {
    Err("Browser screenshots are currently available on Linux only.".into())
}
