//! 协议 → 请求路径的端到端回归。
//!
//! 背景：rig 0.42 的 `openai::Client` 默认走 **Responses API**（`{baseUrl}/responses`），
//! 而 Pipi 的 `openai-completions` 协议承诺的是 Chat Completions 兼容层 —— 中转商与
//! 本地运行时（ollama / vLLM / DeepSeek / 硅基流动…）只实现 `/chat/completions`。
//! 所以 provider.rs 必须显式 `completions_api()`。
//!
//! 做法：把供应商端点指向必然拒绝连接的本地端口，跑一轮真实 agent，再从会话 jsonl 里
//! 读回「实际请求的 URL」——不靠 mock，也不用真密钥。
//!
//! 本测试自己改写 HOME（独立测试进程），不会碰用户真实的 `~/.pipi`。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipi_core::agents;
use pipi_core::runtime::{EventEmitter, RuntimeState};
use pipi_core::settings::{save_settings, ProviderConfig, Settings, Theme};
use pipi_core::types::{Api, Model};

fn temp_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!("pipi-request-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("创建临时 HOME");
    std::env::set_var("HOME", &home);
    home
}

/// 播种一个绑定到 `(api, base_url)` 的 Agent + 对应供应商设置。
fn fixture(agent: &str, api: Api, base_url: &str) {
    let model = Model {
        id: "regression-model".into(),
        name: "Regression Model".into(),
        api,
        base_url: base_url.into(),
        max_tokens: 512,
        context_window: 8192,
    };
    agents::create_agent(
        agent,
        "协议路径回归",
        None,
        None,
        Some("regression-model"),
        Some(model),
    )
    .expect("播种测试 Agent");
    save_settings(&Settings {
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "refusing".into(),
            name: "Refusing Endpoint".into(),
            api,
            base_url: base_url.into(),
            env_key: None,
            api_key: Some("test-key".into()),
        }],
        default_provider_id: None,
        compaction: Default::default(),
    })
    .expect("写入测试设置");
}

/// 跑一轮并等它失败，返回该会话 jsonl 的全文（里面记录了实际请求的 URL）。
fn send_and_collect(agent: &str, home: &Path) -> String {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("构建注入运行时");
    let state = RuntimeState::new(runtime.handle().clone());
    let sink: EventEmitter = Arc::new(|_| {});

    state
        .send_prompt(agent, "hello", None, sink)
        .expect("send_prompt 应能启动这一轮");

    let sessions = home.join(".pipi").join("agents").join(agent).join("sessions");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut text = String::new();
    while Instant::now() < deadline {
        text = std::fs::read_dir(&sessions)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().extension().map(|e| e == "jsonl").unwrap_or(false))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect::<Vec<_>>()
            .join("\n");
        if text.contains("\"stopReason\":\"error\"") {
            return text;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("这一轮没有在 20s 内落到错误状态；已记录内容：{text}");
}

/// 从「errorMessage 里的 URL」抽出来便于断言（会话 jsonl 是转义过的 JSON 字符串）。
fn request_url(text: &str) -> Option<String> {
    let start = text.find("for url (")? + "for url (".len();
    let rest = &text[start..];
    let end = rest.find(')')?;
    Some(rest[..end].to_string())
}

#[test]
fn 两种协议各自打到兼容端点() {
    let home = temp_home();

    // ---- openai-completions → /chat/completions（不是 /responses）----
    fixture("path-openai", Api::OpenAICompletions, "http://127.0.0.1:1/v1");
    let openai_text = send_and_collect("path-openai", &home);
    let url = request_url(&openai_text).unwrap_or_default();
    assert!(
        url.ends_with("/v1/chat/completions"),
        "openai-completions 必须打 /chat/completions，实际：{url}"
    );
    assert!(
        !openai_text.contains("/responses"),
        "不应再出现 Responses API：{openai_text}"
    );

    // ---- anthropic-messages → /v1/messages ----
    fixture("path-anthropic", Api::AnthropicMessages, "http://127.0.0.1:1");
    let anthropic_text = send_and_collect("path-anthropic", &home);
    let url = request_url(&anthropic_text).unwrap_or_default();
    assert!(
        url.ends_with("/v1/messages"),
        "anthropic-messages 必须打 /v1/messages，实际：{url}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
