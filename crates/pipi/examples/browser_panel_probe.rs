//! Exercises the real React panel against Tauri's child-WebView API.
#[cfg(target_os = "linux")]
static LAYOUT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(target_os = "linux")]
#[tauri::command]
fn probe_sensitive(state: tauri::State<'_, std::sync::Arc<std::sync::atomic::AtomicUsize>>) -> &'static str {
    state.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    "ok"
}

#[cfg(target_os = "linux")]
#[tauri::command]
async fn browser_view(
    state: tauri::State<'_, std::sync::Arc<pipi_lib::browser::DesktopBrowser>>,
    agent_name: String,
    session_id: String,
    action: String,
    url: Option<String>,
    bounds: Option<pipi_core::tools::browser::BrowserBounds>,
) -> Result<pipi_core::tools::browser::BrowserViewState, String> {
    use pipi_core::tools::browser::BrowserBackend;
    if action == "layout" {
        LAYOUT_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    state
        .ui(&agent_name, &session_id, &action, url.as_deref(), bounds, &state.lease_session(&agent_name, &session_id))
        .await
}

#[cfg(target_os = "linux")]
fn main() {
    use pipi_core::tools::browser::{BrowserBackend, BrowserTool};
    use pipi_protocol::{
        AbortSignal, ToolResultContent,
        permissions::{PermissionsConfig, SandboxMode},
        tools::{AgentTool, ToolContext},
    };
    use std::time::Duration;
    use std::{collections::BTreeMap, sync::Arc};
    use tauri::Manager;
    let sensitive_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct ExitOnPanic(tauri::AppHandle);
    impl Drop for ExitOnPanic {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.exit(1);
            }
        }
    }
    tauri::Builder::default()
        .manage(sensitive_calls.clone())
        .invoke_handler(|invoke| {
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![browser_view, probe_sensitive];
            pipi_lib::restrict_invoke(invoke, handler)
        })
        .setup(move |app| {
            let main = app.get_webview("main").ok_or("missing main webview")?;
            main.navigate("http://localhost:1420/tests/browser-panel.browser.html".parse()?)?;
            let handle = app.handle().clone();
            let browser = Arc::new(pipi_lib::browser::DesktopBrowser::new(handle.clone()));
            app.manage(browser.clone());
            std::thread::spawn(move || {
                let _exit_on_panic = ExitOnPanic(handle.clone());
                std::thread::sleep(Duration::from_secs(4));
                let tool = BrowserTool::new(browser.clone(), "test-agent".into(), "test-session".into());
                let other_tool = BrowserTool::new(browser.clone(), "test-agent".into(), "other-session".into());
                let context = ToolContext { workspace: "/tmp".into(), memory_dir: None, notebook_path: None, read_roots: vec![], permissions: Arc::new(PermissionsConfig { tools: vec!["browser".into()], sandbox: SandboxMode::DangerFullAccess }), sandbox: SandboxMode::DangerFullAccess, resolved_env: Arc::new(BTreeMap::new()), abort: AbortSignal::new() };
                tauri::async_runtime::block_on(async {
                    assert!(tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"file:///etc/passwd"}), &|_| {}).await.is_err());
                    assert_eq!(handle.webviews().keys().filter(|label| label.starts_with("browser-")).count(), 0);
                    let navigation = serde_json::json!({"action":"navigate", "url":"http://localhost:1420/tests/browser-panel.browser.html"});
                    let lease = browser.lease_session("test-agent", "test-session");
                    let (agent_result, ui_result) = tokio::join!(
                        tool.execute(&context, &navigation, &|_| {}),
                        browser.ui("test-agent", "test-session", "navigate", Some("http://localhost:1420/tests/browser-panel.browser.html"), None, &lease),
                    );
                    agent_result.expect("agent creates browser");
                    ui_result.expect("concurrent user navigation");
                    assert_eq!(handle.webviews().keys().filter(|label| label.starts_with("browser-")).count(), 1);
                    println!("CONCURRENT_NAVIGATION_REUSES_VIEW");
                    println!("AGENT_CREATED_BROWSER");
                });
                std::thread::sleep(Duration::from_secs(3));
                let (tx, rx) = std::sync::mpsc::channel();
                main.eval_with_callback("[!document.querySelector('.browser-toolbar button:last-child').disabled, document.querySelector('.browser-toolbar input').value]", move |value| { let _ = tx.send(value); }).unwrap();
                let attached: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
                assert_eq!(attached[0], true);
                assert!(attached[1].as_str().unwrap().contains("browser-panel.browser.html"));
                println!("PANEL_ATTACHED_TO_AGENT_BROWSER");
                main.eval(r#"(()=>{const input=document.querySelector('input'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,'http://localhost:1420/tests/browser-panel.browser.html'); input.dispatchEvent(new Event('input',{bubbles:true})); setTimeout(()=>input.form.requestSubmit(),100)})()"#).expect("submit fixture");
                std::thread::sleep(Duration::from_secs(4));
                assert_eq!(handle.webviews().keys().filter(|label| label.starts_with("browser-")).count(), 1);
                let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-"));
                match child {
                    Some((label, webview)) => {
                        let _ = main.eval_with_callback("JSON.stringify({rect:document.querySelector('.browser-viewport').getBoundingClientRect().toJSON(), error:document.querySelector('.file-notice')?.innerText})", |value| println!("PANEL_LAYOUT {value}"));
                        println!("PANEL_CREATED {label}");
                        println!("CHILD_NATIVE_SIZE {:?}", webview.size());
                        let (tx, rx) = std::sync::mpsc::channel();
                        webview.with_webview(move |native| {
                            use gtk::prelude::*;
                            let child = native.inner();
                            let fixed = child.parent().unwrap().downcast::<gtk::Fixed>().unwrap();
                            let overlay = fixed.parent().unwrap().downcast::<gtk::Overlay>().unwrap();
                            let _ = tx.send((child.has_window(), fixed.has_window(), overlay.is_overlay_pass_through(&fixed), fixed.window().unwrap().is_pass_through(), child.window().unwrap().is_pass_through()));
                        }).unwrap();
                        let input_layer = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        assert_eq!(input_layer, (true, false, true, true, false), "background overlay must not intercept input and the child must remain interactive");
                        println!("INPUT_LAYER_PROPERTIES {input_layer:?}");
                        let (tx, rx) = std::sync::mpsc::channel();
                        main.eval_with_callback("[document.querySelector('.browser-viewport').getBoundingClientRect().width,document.querySelector('.browser-viewport').getBoundingClientRect().height]", move |value| { let _ = tx.send(value); }).unwrap();
                        let expected: Vec<f64> = serde_json::from_str(&rx.recv().unwrap()).unwrap();
                        let (tx, rx) = std::sync::mpsc::channel();
                        webview.eval_with_callback("[window.innerWidth,window.innerHeight]", move |value| { let _ = tx.send(value); }).unwrap();
                        let actual: Vec<f64> = serde_json::from_str(&rx.recv().unwrap()).unwrap();
                        assert!((expected[0] - actual[0]).abs() <= 2.0 && (expected[1] - actual[1]).abs() <= 2.0, "viewport {actual:?}, panel {expected:?}");
                        println!("PANEL_VIEWPORT_MATCHES");

                        let _ = webview.eval_with_callback("[window.innerWidth,window.innerHeight]", |size| println!("CHILD_VIEWPORT_SIZE {size}"));
                        let _ = webview.eval_with_callback("document.title", |title| println!("PANEL_TITLE {title}"));
                    }
                    None => println!("PANEL_MISSING"),
                }
                let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-")).unwrap().1;
                let probe = "(()=>{window.__probe={command:'pending',plugin:'pending'};if(!window.__TAURI_INTERNALS__){window.__probe={command:'absent',plugin:'absent'};return}window.__TAURI_INTERNALS__.invoke('probe_sensitive').then(()=>window.__probe.command='allowed',()=>window.__probe.command='denied');window.__TAURI_INTERNALS__.invoke('plugin:app|name').then(()=>window.__probe.plugin='allowed',()=>window.__probe.plugin='denied')})()";
                main.eval(probe).unwrap();
                child.eval(probe).unwrap();
                std::thread::sleep(Duration::from_secs(2));
                let read_probe = |view: &tauri::Webview| {
                    let (tx, rx) = std::sync::mpsc::channel();
                    view.eval_with_callback("window.__probe", move |v| { let _ = tx.send(v); }).unwrap();
                    serde_json::from_str::<serde_json::Value>(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
                };
                let trusted = read_probe(&main);
                let untrusted = read_probe(&child);
                assert_eq!(trusted["command"], "allowed");
                assert_eq!(trusted["plugin"], "allowed");
                assert!(matches!(untrusted["command"].as_str(), Some("denied" | "absent")));
                assert!(matches!(untrusted["plugin"].as_str(), Some("denied" | "absent")));
                assert_eq!(sensitive_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
                println!("CHILD_WEBVIEW_IPC_RESTRICTED");
                if std::env::var_os("PIPI_TEST_LAYOUT_FLOOD").is_some() {
                    use std::sync::atomic::Ordering;
                    let before = LAYOUT_CALLS.load(Ordering::Relaxed);
                    main.eval(r#"(()=>{const el=document.createElement('div');document.body.appendChild(el);let n=0;let id=setInterval(()=>{el.replaceChildren(document.createTextNode(String(n++)))},8);setTimeout(()=>{clearInterval(id);el.remove()},3000)})()"#).unwrap();
                    std::thread::sleep(Duration::from_secs(4));
                    let count = LAYOUT_CALLS.load(Ordering::Relaxed) - before;
                    assert!(count <= 2, "unrelated page updates triggered {count} browser layouts");
                    println!("LAYOUT_FLOOD_CALLS {count}");
                    let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-")).unwrap().1;
                    main.eval("(()=>{const d=document.createElement('div');d.id='probe-modal';d.setAttribute('role','dialog');d.setAttribute('aria-modal','true');document.body.appendChild(d)})()").unwrap();
                    std::thread::sleep(Duration::from_secs(1));
                    let (tx, rx) = std::sync::mpsc::channel();
                    child.with_webview(move |native| { use gtk::prelude::*; let _ = tx.send(native.inner().is_visible()); }).unwrap();
                    assert!(!rx.recv_timeout(Duration::from_secs(5)).unwrap(), "browser must hide behind an open dialog");
                    main.eval("document.querySelector('#probe-modal').remove()").unwrap();
                    std::thread::sleep(Duration::from_secs(1));
                    let (tx, rx) = std::sync::mpsc::channel();
                    child.with_webview(move |native| { use gtk::prelude::*; let _ = tx.send(native.inner().is_visible()); }).unwrap();
                    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "browser must reappear after the dialog closes");
                    println!("MODAL_VISIBILITY_OK");
                    let (tx, rx) = std::sync::mpsc::channel();
                    main.eval_with_callback("Math.round(document.querySelector('.browser-viewport').getBoundingClientRect().top)", move |v| { let _ = tx.send(v); }).unwrap();
                    let previous_viewport: i32 = serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
                    let (tx, rx) = std::sync::mpsc::channel();
                    child.with_webview(move |native| {
                        use gtk::prelude::*;
                        let widget = native.inner();
                        let root = widget.toplevel().unwrap();
                        let _ = tx.send(widget.translate_coordinates(&root, 0, 0).unwrap().1);
                    }).unwrap();
                    let previous_native = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    main.eval("(()=>{const p=document.querySelector('.browser-panel');const d=document.createElement('div');d.id='probe-spacer';d.style.height='20px';p.parentElement.insertBefore(d,p)})()").unwrap();
                    std::thread::sleep(Duration::from_secs(1));
                    let (tx, rx) = std::sync::mpsc::channel();
                    main.eval_with_callback("Math.round(document.querySelector('.browser-viewport').getBoundingClientRect().top)", move |v| { let _ = tx.send(v); }).unwrap();
                    let expected: i32 = serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
                    let (tx, rx) = std::sync::mpsc::channel();
                    child.with_webview(move |native| {
                        use gtk::prelude::*;
                        let widget = native.inner();
                        let root = widget.toplevel().unwrap();
                        let _ = tx.send(widget.translate_coordinates(&root, 0, 0).unwrap().1);
                    }).unwrap();
                    let actual = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert_eq!(expected - previous_viewport, 20, "fixture did not move the viewport");
                    assert!(((expected - previous_viewport) - (actual - previous_native)).abs() <= 2, "browser moved {} while viewport moved {}", actual - previous_native, expected - previous_viewport);
                    main.eval("document.querySelector('#probe-spacer').remove()").unwrap();
                    std::thread::sleep(Duration::from_secs(1));
                    println!("VIEWPORT_POSITION_CHANGE_OK");
                }
                tauri::async_runtime::block_on(async {
                    assert!(other_tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.is_err());
                    println!("OTHER_SESSION_REJECTED");
                    let result = tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.expect("snapshot tool");
                    let ToolResultContent::Text { text } = &result.content[0] else { panic!("expected text") };
                    let data: serde_json::Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
                    assert!(data["text"].as_str().unwrap().contains("Unique page body for browser inspection."));
                    println!("TOOL_BODY_OK");
                    let fill_id = data["elements"].as_array().unwrap().iter().find(|e| e["text"] == "Test input").unwrap()["id"].as_str().unwrap();
                    tool.execute(&context, &serde_json::json!({"action":"fill", "element":fill_id, "value":"browser test"}), &|_| {}).await.expect("fill tool");
                    let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-")).unwrap().1;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let tx = std::sync::Mutex::new(Some(tx));
                    child.eval_with_callback("document.querySelector('[aria-label=\"Test input\"]').value", move |value| { if let Some(tx) = tx.lock().unwrap().take() { let _ = tx.send(value); } }).unwrap();
                    assert_eq!(rx.await.unwrap(), "\"browser test\"");
                    println!("TOOL_FILL_CHANGED_PAGE");
                    let selected = tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.unwrap();
                    let ToolResultContent::Text { text } = &selected.content[0] else { panic!("expected text") };
                    let selected: serde_json::Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
                    let select_id = selected["elements"].as_array().unwrap().iter().find(|e| e["text"] == "Choice").unwrap()["id"].as_str().unwrap();
                    tool.execute(&context, &serde_json::json!({"action":"select", "element":select_id, "value":"two"}), &|_| {}).await.expect("select tool");
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let tx = std::sync::Mutex::new(Some(tx));
                    child.eval_with_callback("document.querySelector('select').value", move |value| { if let Some(tx) = tx.lock().unwrap().take() { let _ = tx.send(value); } }).unwrap();
                    assert_eq!(rx.await.unwrap(), "\"two\"");
                    tool.execute(&context, &serde_json::json!({"action":"scroll", "direction":"down"}), &|_| {}).await.expect("scroll tool");
                    println!("SELECT_AND_SCROLL_OK");
                    tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"http://localhost:1420/tests/browser-eval/?page=request"}), &|_| {}).await.expect("open form");
                    let form = tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.unwrap();
                    let ToolResultContent::Text { text } = &form.content[0] else { panic!("expected text") };
                    let form: serde_json::Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
                    let element = |label: &str| form["elements"].as_array().unwrap().iter().find(|e| e["text"] == label).unwrap()["id"].as_str().unwrap().to_owned();
                    let name = element("申请人");
                    let kind = element("申请类型");
                    let reason = element("说明");
                    let submit = element("提交申请");
                    tool.execute(&context, &serde_json::json!({"action":"fill", "element":name, "value":"陈星"}), &|_| {}).await.expect("fill first field");
                    assert!(tool.execute(&context, &serde_json::json!({"action":"fill", "element":name, "value":"duplicate"}), &|_| {}).await.is_err(), "used reference must be consumed");
                    tool.execute(&context, &serde_json::json!({"action":"select", "element":kind, "value":"equipment"}), &|_| {}).await.expect("select from same snapshot");
                    tool.execute(&context, &serde_json::json!({"action":"fill", "element":reason, "value":"测试工位显示器"}), &|_| {}).await.expect("fill second field from same snapshot");
                    let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-")).unwrap().1;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let tx = std::sync::Mutex::new(Some(tx));
                    child.eval_with_callback("[document.querySelector('[name=name]').value,document.querySelector('select').value,document.querySelector('textarea').value]", move |value| { if let Some(tx) = tx.lock().unwrap().take() { let _ = tx.send(value); } }).unwrap();
                    let fields: Vec<String> = serde_json::from_str(&rx.await.unwrap()).unwrap();
                    assert_eq!(fields, ["陈星", "equipment", "测试工位显示器"]);
                    child.eval("document.querySelector('h2').textContent='Changed context'").unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    assert!(tool.execute(&context, &serde_json::json!({"action":"click", "element":submit}), &|_| {}).await.is_err(), "DOM changes must invalidate remaining references");
                    println!("FORM_REFERENCES_AND_MUTATION_GUARD_OK");
                    tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"http://127.0.0.1:1420/tests/browser-eval/?page=diagnosis"}), &|_| {}).await.expect("open cross-origin frame");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let frame = tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.unwrap();
                    let ToolResultContent::Text { text } = &frame.content[0] else { panic!("expected text") };
                    let frame: serde_json::Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
                    assert!(frame["unavailableFrames"].as_array().unwrap().iter().any(|entry| entry["title"] == "独立诊断窗口"));
                    assert!(!frame["elements"].as_array().unwrap().iter().any(|entry| entry["text"] == "启动诊断"));
                    println!("CROSS_ORIGIN_LIMIT_REPORTED");
                    tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"http://localhost:1420/tests/browser-panel.browser.html"}), &|_| {}).await.expect("restore fixture");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let result = tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.expect("fresh snapshot");
                    let ToolResultContent::Text { text } = &result.content[0] else { panic!("expected text") };
                    let data: serde_json::Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
                    let id = data["elements"].as_array().unwrap().iter().find(|e| e["text"] == "Toggle").unwrap()["id"].as_str().unwrap();
                    println!("TOOL_SNAPSHOT_OK");
                    let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-")).unwrap().1;
                    child.eval("window.__pipiBrowserState = {refs:{}, stale:true}; Element.prototype.click = () => {};").unwrap();
                    println!("PAGE_WORLD_TAMPERED");
                    tool.execute(&context, &serde_json::json!({"action":"click", "element":id}), &|_| {}).await.expect("click tool");
                    println!("TOOL_CLICK_OK");
                    let child = handle.webviews().into_iter().find(|(label, _)| label.starts_with("browser-")).unwrap().1;
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    let sender = std::sync::Mutex::new(Some(sender));
                    child.eval_with_callback("document.querySelector('#toggle').getAttribute('data-active')", move |value| { if let Some(sender) = sender.lock().unwrap().take() { let _ = sender.send(value); } }).unwrap();
                    let state = receiver.await.unwrap();
                    assert_eq!(state, "\"false\"");
                    println!("TOOL_CLICK_CHANGED_PAGE");
                    assert!(tool.execute(&context, &serde_json::json!({"action":"click", "element":id}), &|_| {}).await.is_err());
                    println!("TOOL_STALE_REJECTED");
                    let screenshot = tool.execute(&context, &serde_json::json!({"action":"screenshot"}), &|_| {}).await.expect("screenshot tool");
                    assert!(matches!(&screenshot.content[0], ToolResultContent::Image { data, mime_type } if mime_type == "image/png" && data.starts_with("iVBORw0KGgo")));
                    println!("TOOL_SCREENSHOT_OK");
                    assert!(tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"file:///etc/passwd"}), &|_| {}).await.is_err());
                    let failed = tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"http://127.0.0.1:1/"}), &|_| {}).await;
                    println!("NAVIGATION_FAILURE_RESULT {:?}", failed);
                    assert!(failed.is_err());
                    println!("NAVIGATION_FAILURE_OK");
                    tool.execute(&context, &serde_json::json!({"action":"navigate", "url":"http://localhost:1420/tests/browser-panel.browser.html?agent=1"}), &|_| {}).await.expect("navigate tool");
                    let state = browser.ui("test-agent", "test-session", "state", None, None, &browser.lease_session("test-agent", "test-session")).await.unwrap();
                    assert!(state.url.contains("?agent=1"));
                    println!("NAVIGATION_STATE_OK");
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let tx = std::sync::Mutex::new(Some(tx));
                    child.eval_with_callback("location.search", move |value| { if let Some(tx) = tx.lock().unwrap().take() { let _ = tx.send(value); } }).unwrap();
                    assert_eq!(rx.await.unwrap(), "\"?agent=1\"");
                    println!("TOOL_NAVIGATE_OK");
                });
                main.eval("document.querySelector('#toggle').click()").expect("hide panel");
                std::thread::sleep(Duration::from_secs(1));
                main.eval("document.querySelector('#toggle').click()").expect("show panel");
                std::thread::sleep(Duration::from_secs(1));
                println!("PANEL_AFTER_TOGGLE {}", handle.webviews().keys().filter(|label| label.starts_with("browser-")).count());
                main.navigate("http://localhost:1420/tests/browser-panel.browser.html?reload=1".parse().unwrap()).unwrap();
                std::thread::sleep(Duration::from_secs(3));
                let (tx, rx) = std::sync::mpsc::channel();
                main.eval_with_callback("[!document.querySelector('.browser-toolbar button:last-child').disabled, document.querySelector('.browser-toolbar input').value]", move |value| { let _ = tx.send(value); }).unwrap();
                let recovered: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
                assert_eq!(recovered[0], true);
                assert!(recovered[1].as_str().unwrap().contains("?agent=1"));
                println!("PANEL_REATTACHED_AFTER_RELOAD");
                main.eval("document.querySelector('#unmount').click()").expect("unmount panel");
                std::thread::sleep(Duration::from_secs(2));
                assert_eq!(handle.webviews().keys().filter(|label| label.starts_with("browser-")).count(), 1);
                println!("PANEL_PRESERVED_AFTER_UNMOUNT");
                tauri::async_runtime::block_on(async {
                    browser.rekey_session("test-agent", "test-session", "forked-session");
                    let forked = BrowserTool::new(browser.clone(), "test-agent".into(), "forked-session".into());
                    assert!(forked.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.is_ok());
                    let image = forked.execute(&context, &serde_json::json!({"action":"screenshot"}), &|_| {}).await.expect("background screenshot");
                    assert!(matches!(&image.content[0], ToolResultContent::Image { mime_type, .. } if mime_type == "image/png"));
                    println!("BACKGROUND_SCREENSHOT_OK");
                    assert!(tool.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.is_err());
                    println!("BROWSER_REKEY_OK");
                    browser.close_session("test-agent", "forked-session");
                    assert!(forked.execute(&context, &serde_json::json!({"action":"snapshot"}), &|_| {}).await.is_err());
                });
                std::thread::sleep(Duration::from_secs(1));
                assert_eq!(handle.webviews().keys().filter(|label| label.starts_with("browser-")).count(), 0);
                println!("CLOSED_REKEYED_BROWSER");

                // Block the GUI thread while creation is queued. Revocation must
                // return promptly and a new incarnation must not be closed by
                // the stale creation once the GUI becomes available again.
                let (ready_tx, ready_rx) = std::sync::mpsc::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                handle.run_on_main_thread(move || {
                    ready_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }).unwrap();
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                tauri::async_runtime::block_on(async {
                    let stale = browser.lease_session("test-agent", "pending-session");
                    let pending_browser = browser.clone();
                    let pending_lease = stale.clone();
                    let pending = tokio::spawn(async move {
                        pending_browser.ui("test-agent", "pending-session", "navigate", Some("http://localhost:1420/tests/browser-panel.browser.html"), None, &pending_lease).await
                    });
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    let started = std::time::Instant::now();
                    browser.close_session("test-agent", "pending-session");
                    assert!(started.elapsed() < Duration::from_secs(1), "cleanup waited for GUI creation");
                    assert!(!stale.is_valid());
                    let fresh = browser.lease_session("test-agent", "pending-session");
                    let restored_browser = browser.clone();
                    let restored_lease = fresh.clone();
                    let restored = tokio::spawn(async move {
                        restored_browser.ui("test-agent", "pending-session", "navigate", Some("http://localhost:1420/tests/browser-panel.browser.html"), None, &restored_lease).await
                    });
                    release_tx.send(()).unwrap();
                    assert!(tokio::time::timeout(Duration::from_secs(15), pending).await.unwrap().unwrap().is_err());
                    let restored = tokio::time::timeout(Duration::from_secs(15), restored).await.unwrap().unwrap().unwrap();
                    assert!(fresh.is_valid());
                    assert!(handle.get_webview(&restored.label).is_some());
                    println!("PENDING_CREATION_REVOKED_WITHOUT_CLOSING_RESTORED_VIEW");
                    browser.close_session("test-agent", "pending-session");
                    let newer = browser.lease_session("test-agent", "pending-session");
                    let newer_view = browser.ui("test-agent", "pending-session", "navigate", Some("http://localhost:1420/tests/browser-panel.browser.html"), None, &newer).await.unwrap();
                    assert_ne!(restored.label, newer_view.label, "a closing view must not be adopted by a restored session");
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    assert!(handle.get_webview(&newer_view.label).is_some());
                    println!("RESTORED_SESSION_DOES_NOT_REUSE_CLOSING_VIEW");
                    let other = browser.lease_session("other-agent", "other-session");
                    let other_view = browser.ui("other-agent", "other-session", "navigate", Some("http://localhost:1420/tests/browser-panel.browser.html"), None, &other).await.unwrap();
                    browser.close_agent("test-agent");
                    assert!(!newer.is_valid());
                    assert!(other.is_valid());
                    assert!(handle.get_webview(&other_view.label).is_some());
                    browser.close_agent("other-agent");
                    println!("AGENT_CLEANUP_ISOLATED");
                });
                main.eval("document.querySelector('#unmount').click()").expect("remount panel");
                std::thread::sleep(Duration::from_secs(1));
                handle.exit(0);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("panel probe failed");
}
#[cfg(not(target_os = "linux"))]
fn main() {}
