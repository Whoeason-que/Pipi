//! 协议协商的端到端回归：网关按模型分协议时，用户不需要配置协议。
//!
//! 覆盖两层（对应 `pipi_provider::effective_api` 的解析顺序）：
//! 1. **目录层**：本地目录缓存里的模型级协议覆盖（上游 models.dev 的
//!    `model.provider.npm`）让 run 的**第一次**请求就走对协议 —— auto 模式、
//!    绑定里声明的还是旧协议，用户什么都不用改；
//! 2. **运行期协商**：目录不知道时，端点回「模型不支持这个协议」→ agent loop
//!    换另一种协议重试一次（不占重试预算），成功后记进进程内记忆 ——
//!    下一次 run 直接走对的协议，不再试探；
//! 3. **钉死协议**（manual）：不换协议，错误原样透出并提示改为「自动」。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipi_core::events::{EventEmitter, RuntimeEvent};
use pipi_core::runtime::RuntimeState;
use pipi_core::settings::{ApiMode, ProviderConfig, Settings, Theme, save_settings};
use pipi_protocol::{Api, Model};
use pipi_venv::agents;
use serde_json::json;

/// 本文件的用例都改写进程级 `HOME`，必须串行（同 `concurrent_sessions.rs`）。
use pipi_test_support::HOME_LOCK;

const MODEL_ID: &str = "mock-gateway-model";
/// opencode 网关的原话：模型只走另一套 wire 时它就是这么回的。
const PROTOCOL_MISMATCH_BODY: &str =
    r#"{"type":"error","error":{"type":"ModelProtocolUnsupported","message":"Model does not support this protocol."}}"#;

// ---------------------------------------------------------------------------
// mock 端点：/chat/completions 与 /v1/messages 两条路径各自可配「拒绝 / 正常」
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// 200 + 该路径对应的正常流（chat = OpenAI SSE，messages = Anthropic SSE）
    Ok,
    /// 400 + `PROTOCOL_MISMATCH_BODY`
    ProtocolUnsupported,
}

struct MockEndpoint {
    base_url: String,
    chat_hits: Arc<AtomicUsize>,
    messages_hits: Arc<AtomicUsize>,
}

impl MockEndpoint {
    fn chat_hits(&self) -> usize {
        self.chat_hits.load(Ordering::SeqCst)
    }

    fn messages_hits(&self) -> usize {
        self.messages_hits.load(Ordering::SeqCst)
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) => {
                buffer.push(byte[0]);
                if buffer.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            _ => return String::new(),
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_string();
    let path = head
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let content_length = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    let _ = stream.read_exact(&mut body);
    path
}

fn respond(stream: &mut std::net::TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// OpenAI Chat Completions 的最小正常流（形状与 concurrent_sessions.rs 一致）。
fn openai_stream(text: &str) -> String {
    let start = json!({
        "id": "chatcmpl-text",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let end = json!({
        "id": "chatcmpl-text",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

/// Anthropic Messages 的最小正常流（形状抄自 rig-core 的 anthropic streaming 测试）。
fn anthropic_stream(text: &str) -> String {
    let events = [
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"mock","content":[],"stop_reason":null,"usage":{"input_tokens":9,"output_tokens":1,"cache_read_input_tokens":0}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ];
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

/// 起一个双协议端点：chat（`/chat/completions`）与 messages（`/v1/messages`）
/// 各自决定是拒绝还是正常返回；两条路径的命中次数分别记账。
fn spawn_protocol_endpoint(chat: Reply, messages: Reply) -> MockEndpoint {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let chat_hits = Arc::new(AtomicUsize::new(0));
    let messages_hits = Arc::new(AtomicUsize::new(0));
    let chat_counter = chat_hits.clone();
    let messages_counter = messages_hits.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let path = read_request(&mut stream);
            if path.ends_with("/chat/completions") {
                chat_counter.fetch_add(1, Ordering::SeqCst);
                match chat {
                    Reply::Ok => respond(
                        &mut stream,
                        "200 OK",
                        "text/event-stream",
                        &openai_stream("来自 OpenAI 协议的回复"),
                    ),
                    Reply::ProtocolUnsupported => respond(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        PROTOCOL_MISMATCH_BODY,
                    ),
                }
            } else if path.ends_with("/messages") {
                messages_counter.fetch_add(1, Ordering::SeqCst);
                match messages {
                    Reply::Ok => respond(
                        &mut stream,
                        "200 OK",
                        "text/event-stream",
                        &anthropic_stream("来自 Anthropic 协议的回复"),
                    ),
                    Reply::ProtocolUnsupported => respond(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        PROTOCOL_MISMATCH_BODY,
                    ),
                }
            } else {
                respond(&mut stream, "404 Not Found", "text/plain", "unknown path");
            }
        }
    });
    MockEndpoint {
        base_url: format!("http://{addr}/v1"),
        chat_hits,
        messages_hits,
    }
}

// ---------------------------------------------------------------------------
// 环境与驱动
// ---------------------------------------------------------------------------

/// 隔离的 HOME（settings / cache / agents 都落在里面）。
fn isolated_home(prefix: &str) -> PathBuf {
    let home = pipi_test_support::temp_dir(prefix);
    pipi_test_support::set_env_var("HOME", &home);
    home
}

/// 播种一个绑定到指定端点的 Agent（声明协议故意写 openai：目录/协商要能纠正它）。
fn fixture_agent(name: &str, base_url: &str) {
    let model = Model {
        id: MODEL_ID.into(),
        name: "Mock Gateway".into(),
        api: Api::OpenAICompletions,
        base_url: base_url.into(),
        max_tokens: 256,
        context_window: 0,
    };
    agents::create_agent(name, "协议协商回归", None, None, Some(MODEL_ID), Some(model))
        .expect("播种测试 Agent");
}

fn write_settings(base_url: &str, api_mode: ApiMode) {
    save_settings(&Settings {
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "mock-gateway".into(),
            name: "Mock Gateway".into(),
            api: Api::OpenAICompletions,
            api_mode,
            base_url: base_url.into(),
            env_key: None,
            api_key: Some("test-key".into()),
        }],
        default_provider_id: None,
        compaction: Default::default(),
        retry: Default::default(),
    })
    .expect("写入测试设置");
}

/// 写一份最小目录缓存：该端点上这个模型的**模型级协议**是 `model_api`。
fn write_catalog_cache(home: &std::path::Path, base_url: &str, model_api: &str) {
    let path = home.join(".pipi").join("cache").join("models.json");
    std::fs::create_dir_all(path.parent().expect("缓存目录")).expect("创建缓存目录");
    let catalog = json!({
        "fetchedAt": 0,
        "source": "cache",
        "stale": false,
        "providers": [{
            "id": "mock-gateway",
            "name": "Mock Gateway",
            "api": "openai-completions",
            "baseUrl": base_url,
            "group": "聚合网关",
            "local": false,
            "models": [{ "id": MODEL_ID, "name": "Mock Gateway", "api": model_api }],
        }],
    });
    std::fs::write(&path, serde_json::to_string(&catalog).expect("序列化目录")).expect("写入缓存");
}

fn recording_sink() -> EventEmitter {
    let sink: EventEmitter = Arc::new(|_event: RuntimeEvent| {});
    sink
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("构建注入运行时")
}

fn wait_until(deadline_ms: u64, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

fn open_session_id(state: &RuntimeState, agent_name: &str) -> String {
    state
        .session_infos()
        .expect("读取会话列表")
        .into_iter()
        .find(|info| info.agent_name == agent_name)
        .map(|info| info.session_id)
        .expect("会话应当已经打开")
}

/// 跑一轮并等它收尾，返回会话文件内容。
fn run_prompt(state: &RuntimeState, agent_name: &str, session_id: Option<&str>, prompt: &str) -> String {
    state
        .send_prompt(agent_name, session_id, prompt, None, recording_sink())
        .unwrap_or_else(|error| panic!("本轮应当被接受：{error}"));
    let id = open_session_id(state, agent_name);
    assert!(
        wait_until(10_000, || !state.session_running(agent_name, &id)),
        "本轮应当在 10s 内收尾"
    );
    let sessions_dir = agents::load_agent(agent_name)
        .expect("加载 Agent")
        .sessions_dir()
        .expect("会话目录");
    std::fs::read_to_string(sessions_dir.join(format!("{id}.jsonl"))).expect("读取会话文件")
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[test]
fn catalog_model_level_protocol_wins_on_the_first_request() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = isolated_home("pipi-protocol-catalog");
    // 端点：OpenAI 协议被拒，Anthropic 协议正常 —— 只有走对协议才可能成功
    let endpoint = spawn_protocol_endpoint(Reply::ProtocolUnsupported, Reply::Ok);
    fixture_agent("catalog-agent", &endpoint.base_url);
    write_settings(&endpoint.base_url, ApiMode::Auto);
    // 目录说：这个模型走 anthropic-messages（绑定里声明的 openai 该被覆盖）
    write_catalog_cache(&home, &endpoint.base_url, "anthropic-messages");

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let session = run_prompt(&state, "catalog-agent", None, "ping");

    assert!(
        session.contains("来自 Anthropic 协议的回复"),
        "目录的模型级协议应当让第一次请求就走对：{session}"
    );
    assert_eq!(endpoint.messages_hits(), 1, "应当只打 Anthropic 端点一次");
    assert_eq!(endpoint.chat_hits(), 0, "不该再去试被拒绝的协议");
}

#[test]
fn runtime_negotiation_switches_once_and_remembers() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = isolated_home("pipi-protocol-negotiation");
    // 没有目录缓存：第一次请求按声明走 OpenAI → 400 → 换协议
    let _ = &home;
    let endpoint = spawn_protocol_endpoint(Reply::ProtocolUnsupported, Reply::Ok);
    fixture_agent("negotiation-agent", &endpoint.base_url);
    write_settings(&endpoint.base_url, ApiMode::Auto);

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let session_id = {
        let first = run_prompt(&state, "negotiation-agent", None, "ping");
        assert!(
            first.contains("来自 Anthropic 协议的回复"),
            "换协议后本轮应当成功：{first}"
        );
        assert_eq!(endpoint.chat_hits(), 1, "第一次请求按声明走 OpenAI（被拒）");
        assert_eq!(endpoint.messages_hits(), 1, "换协议只重试一次");
        open_session_id(&state, "negotiation-agent")
    };

    // 第二轮：进程内记忆已经学到这个 (端点, 模型) 走 Anthropic —— 不再试探
    let second = run_prompt(&state, "negotiation-agent", Some(&session_id), "再 ping");
    assert!(second.contains("来自 Anthropic 协议的回复"), "{second}");
    assert_eq!(
        endpoint.chat_hits(),
        1,
        "学到之后不该再打被拒绝的协议：{second}"
    );
    assert_eq!(endpoint.messages_hits(), 2, "第二轮直接走对的协议");
}

#[test]
fn pinned_protocol_is_not_rewritten() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = isolated_home("pipi-protocol-pinned");
    // 目录就算说该走 Anthropic，钉死（manual）也轮不到它说话
    let endpoint = spawn_protocol_endpoint(Reply::ProtocolUnsupported, Reply::Ok);
    fixture_agent("pinned-agent", &endpoint.base_url);
    write_settings(&endpoint.base_url, ApiMode::Manual);
    write_catalog_cache(&home, &endpoint.base_url, "anthropic-messages");

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let session = run_prompt(&state, "pinned-agent", None, "ping");

    assert_eq!(endpoint.chat_hits(), 1, "钉死的协议照发，不换");
    assert_eq!(endpoint.messages_hits(), 0, "钉死时目录与协商都不参与");
    assert!(
        session.contains("协议已固定"),
        "错误要说明协议被钉死、并提示改为自动：{session}"
    );
    assert!(
        !session.contains("来自 Anthropic 协议的回复"),
        "钉死模式下不该有成功回复：{session}"
    );
}
