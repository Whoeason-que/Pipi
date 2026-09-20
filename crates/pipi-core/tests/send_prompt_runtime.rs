//! 回归：桌面壳的 Tauri command 跑在 GTK 主线程上，那里没有 reactor。
//!
//! 用户可见的故障形态是「一发消息整个应用就 abort」：`RuntimeState::send_prompt`
//! 里若直接 `tokio::spawn`，在没有运行时上下文的线程上会 panic，而 panic 发生在
//! 主线程上 → 进程直接中止（there is no reactor running / panic_cannot_unwind）。
//! 核心必须把这一轮 spawn 到宿主注入的运行时上。
//!
//! 所以这个集成测试**故意不在 `block_on` 里调用** —— 与桌面壳的调用形态一致。
//! 它跑在独立的测试进程里并改写 HOME，因此不会和其它用例争 `~/.pipi`。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipi_core::agents;
use pipi_core::runtime::{EventEmitter, RuntimeEvent, RuntimeState};
use pipi_core::settings::{save_settings, ProviderConfig, Settings, Theme};
use pipi_core::types::{Api, Model};

#[test]
fn send_prompt_runs_without_an_ambient_reactor() {
    // 独立 HOME：绝不碰用户真实的 ~/.pipi
    let home = std::env::temp_dir().join(format!("pipi-spawn-regression-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("创建临时 HOME");
    std::env::set_var("HOME", &home);

    // 端点指向必然拒绝连接的本地端口：这一轮只需「被调度并产生事件」，不碰真网络
    let model = Model {
        id: "regression-model".into(),
        name: "Regression Model".into(),
        api: Api::OpenAICompletions,
        base_url: "http://127.0.0.1:1/v1".into(),
        max_tokens: 512,
        context_window: 8192,
    };
    agents::create_agent(
        "spawn-regression",
        "运行时注入回归",
        None,
        None,
        Some("regression-model"),
        Some(model),
    )
    .expect("播种测试 Agent");

    let settings = Settings {
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "local-refusing".into(),
            name: "Local Refusing Endpoint".into(),
            api: Api::OpenAICompletions,
            base_url: "http://127.0.0.1:1/v1".into(),
            env_key: None,
            api_key: Some("test-key".into()),
        }],
        default_provider_id: None,
    };
    save_settings(&settings).expect("写入测试设置");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("构建注入运行时");
    let state = RuntimeState::new(runtime.handle().clone());

    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: EventEmitter = {
        let events = events.clone();
        Arc::new(move |event: RuntimeEvent| {
            let name = match event {
                RuntimeEvent::AgentEvent(_) => "agent-event",
                RuntimeEvent::SessionStats(_) => "session-stats",
                RuntimeEvent::SessionError(_) => "session-error",
                RuntimeEvent::ApprovalRequest(_) => "approval-request",
            };
            events.lock().unwrap().push(name);
        })
    };

    // 关键断言点：在测试线程（没有 reactor）上调用，等价于桌面壳的 Tauri command。
    // 修复前这一行会 panic（there is no reactor running）并中止整个测试进程。
    state
        .send_prompt("spawn-regression", "hello", None, sink)
        .expect("send_prompt 应把这一轮交给注入的运行时，而不是要求调用线程自带 reactor");

    // 再证明这一轮真的被注入的运行时调度执行了（而不是静默丢失）
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && events.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let seen = events.lock().unwrap().clone();
    assert!(
        seen.contains(&"agent-event"),
        "这一轮没有在注入的运行时上真正启动：{seen:?}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
