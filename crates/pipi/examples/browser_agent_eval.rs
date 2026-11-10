//! Runs real Pipi sessions with a real provider against the local browser fixture.
//! The shell wrapper supplies an isolated PIPI_HOME and the model configuration.
#[cfg(target_os = "linux")]
fn main() {
    use pipi_core::{events::RuntimeEvent, runtime::RuntimeState, tools::browser::BrowserBackend};
    use pipi_protocol::{
        Message,
        permissions::{PermissionsConfig, SandboxMode},
    };
    use std::{
        fs,
        io::Write,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tauri::Manager;

    let reference = std::env::var("PIPI_EVAL_MODEL_SOURCE").expect("PIPI_EVAL_MODEL_SOURCE");
    let reference: pipi_venv::agents::AgentDefinition =
        serde_json::from_slice(&fs::read(reference).expect("read model source"))
            .expect("parse model source");
    let model = reference
        .provider
        .expect("reference agent needs a configured model");
    let output =
        std::path::PathBuf::from(std::env::var("PIPI_EVAL_OUTPUT").expect("PIPI_EVAL_OUTPUT"));
    fs::create_dir_all(&output).expect("create output directory");
    let workspace = std::env::var("PIPI_EVAL_WORKSPACE").expect("PIPI_EVAL_WORKSPACE");
    let agent = format!("browser-eval-{}", std::process::id());
    pipi_venv::agents::create_agent(
        &agent,
        "Browser evaluation",
        Some(&workspace),
        Some(PermissionsConfig {
            tools: vec!["browser".into()],
            sandbox: SandboxMode::DangerFullAccess,
        }),
        Some(&model.id),
        Some(model.clone()),
    )
    .expect("create evaluation agent");
    println!(
        "EVAL_AGENT {agent} MODEL {} OUTPUT {}",
        model.id,
        output.display()
    );

    tauri::Builder::default().setup(move |app| {
        let app_handle = app.handle().clone();
        let runtime = Arc::new(RuntimeState::new(tauri::async_runtime::handle().inner().clone()));
        runtime.start_background_waker();
        let browser = Arc::new(pipi_lib::browser::DesktopBrowser::new(app_handle.clone()));
        runtime.attach_browser(browser.clone()).expect("attach desktop browser");
        std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                let tasks = [
                    ("01-overview", "请打开 http://127.0.0.1:1420/tests/browser-eval/ ，告诉我本周值班负责人和服务窗口。"),
                    ("02-navigation", "请从 http://127.0.0.1:1420/tests/browser-eval/ 找到青岚项目，告诉我负责人、计划上线日期和状态。"),
                    ("03-form", "请在 http://127.0.0.1:1420/tests/browser-eval/ 提交申请：申请人陈星，类型设备借用，说明测试工位显示器。完成后告诉我申请编号。"),
                    ("04-frame", "请从 http://127.0.0.1:1420/tests/browser-eval/ 查找消息服务的运行状态和最近维护时间。"),
                    ("05-cross-origin", "请从 http://127.0.0.1:1420/tests/browser-eval/ 进入独立诊断，在页面里的诊断窗口启动诊断，并告诉我结果编号。"),
                ];
                for (id, prompt) in tasks {
                    let file = fs::File::create(output.join(format!("{id}.events.jsonl"))).expect("event file");
                    let file = Arc::new(Mutex::new(file));
                    let sink: pipi_core::events::EventEmitter = Arc::new(move |event| {
                        let include = match &event {
                            RuntimeEvent::AgentEvent(e) => matches!(e.event,
                                pipi_agent::agent_loop::AgentEvent::ToolExecutionStart { .. }
                                | pipi_agent::agent_loop::AgentEvent::ToolExecutionEnd { .. }
                                | pipi_agent::agent_loop::AgentEvent::MessageEnd { .. }
                                | pipi_agent::agent_loop::AgentEvent::RetryStart { .. }),
                            RuntimeEvent::SessionError(_) | RuntimeEvent::SessionSwitched(_) => true,
                            _ => false,
                        };
                        if include && let Ok(mut f) = file.lock() {
                            let _ = writeln!(f, "{}", serde_json::to_string(&event).unwrap());
                        }
                    });
                    let session = runtime.create_session(&agent).expect("create session");
                    println!("TASK_START {id} SESSION {}", session.session_id);
                    let start = Instant::now();
                    let started = runtime.send_prompt(&agent, Some(&session.session_id), prompt, None, sink);
                    let mut status = match started { Ok(_) => "complete".to_string(), Err(e) => format!("start error: {e}") };
                    if status == "complete" {
                        loop {
                            if !runtime.session_info(&agent, &session.session_id).unwrap().unwrap().running { break; }
                            if start.elapsed() > Duration::from_secs(180) {
                                status = "timeout".into();
                                let _ = runtime.stop_run(&agent, &session.session_id);
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                    }
                    let messages = runtime.session_messages(&agent, &session.session_id).await.unwrap();
                    let answer = messages.iter().rev().find_map(|message| match message {
                        Message::Assistant { content, .. } => Some(content), _ => None,
                    });
                    let state = browser.ui(&agent, &session.session_id, "state", None, None, &browser.lease_session(&agent, &session.session_id)).await;
                    let page = if let Ok(state) = state {
                        if let Some(view) = app_handle.get_webview(&state.label) {
                            let (tx, rx) = tokio::sync::oneshot::channel();
                            let tx = Mutex::new(Some(tx));
                            let _ = view.eval_with_callback("JSON.stringify({url:location.href,submission:document.body.dataset.submission || null,diagnosis:document.body.dataset.diagnosis || null,text:document.body.innerText})", move |value| {
                                if let Some(tx) = tx.lock().unwrap().take() { let _ = tx.send(value); }
                            });
                            tokio::time::timeout(Duration::from_secs(5), rx).await.ok().and_then(Result::ok)
                        } else { None }
                    } else { None };
                    let report = serde_json::json!({"task":id,"agent":agent,"session":session.session_id,"model":model.id,"status":status,"seconds":start.elapsed().as_secs_f64(),"answer":answer,"page":page,"messages":messages});
                    fs::write(output.join(format!("{id}.json")), serde_json::to_vec_pretty(&report).unwrap()).expect("write report");
                    println!("TASK_END {id} STATUS {status} SECONDS {:.1}", start.elapsed().as_secs_f64());
                    browser.close_session(&agent, &session.session_id);
                }
                if let Err(error) = runtime.shutdown().await { eprintln!("shutdown: {error}"); }
                app_handle.exit(0);
            });
        });
        Ok(())
    }).run(tauri::generate_context!()).expect("run browser evaluation");
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The browser agent evaluation currently requires Linux.");
}
