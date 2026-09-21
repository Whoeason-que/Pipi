//! 请求重试的端到端回归：本地 HTTP 端点 → 真实 rig 栈 → 分类 → 重发。
//!
//! 覆盖两件事：
//! 1. **可重试错误（429 / 连接中断）会被重发**，并发出 `retry_start` 事件，最终拿到正常回复；
//! 2. **终态错误（400）不重发** —— 重试请求不合法只会白等。
//!
//! 端点按请求序号回放「完整 HTTP 响应原文」，所以能造出真实的非 200 状态
//! （`agent_composition.rs` 的 `spawn_scripted_mock` 只会回 200）。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use pipi_core::agent_loop::{run_agent_loop, AgentContext, AgentEvent, AgentLoopConfig};
use pipi_core::permissions::SandboxMode;
use pipi_core::provider::provider_for;
use pipi_core::retry::RetryPolicy;
use pipi_core::tools::{ToolContext, ToolRegistry};
use pipi_core::types::{AbortSignal, Api, Message, Model, StopReason, StreamOptions};
use serde_json::json;

/// OpenAI 流式文本回合的 SSE 响应体。
fn sse_text(text: &str) -> String {
    let start = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let end = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

/// 一段完整的 HTTP 响应：状态行、调用方给的响应头、body。
///
/// `Content-Type` 由调用方给 —— 重复的同名头会让 reqwest 取到前一个值，
/// 把 SSE 响应误判成别的内容类型（这正是 rig 的 `InvalidContentType` 来源）。
fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// 读取并丢弃一个 HTTP 请求（保证响应前把 body 读完）。
fn drain_http_request(stream: &mut std::net::TcpStream) {
    use std::io::Read;
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
            _ => return,
        }
    }
    let headers = String::from_utf8_lossy(&buffer).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut remaining = vec![0u8; content_length];
    let _ = stream.read_exact(&mut remaining);
}

/// 按请求序号回放「完整 HTTP 响应」的 mock；返回（地址，请求计数）。
fn spawn_http_script(script: Vec<String>) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let script = Arc::new(script);
    let counter_for_thread = counter.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let script = script.clone();
            let counter = counter_for_thread.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                drain_http_request(&mut stream);
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let response = script
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| {
                        http_response(
                            "200 OK",
                            &[("Content-Type", "text/event-stream")],
                            &sse_text("(mock 剧本已耗尽)"),
                        )
                    });
                use std::io::Write;
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    (addr, counter)
}

fn test_model(addr: std::net::SocketAddr) -> Model {
    Model {
        id: "mock-model".into(),
        name: "Mock".into(),
        api: Api::OpenAICompletions,
        base_url: format!("http://{addr}/v1"),
        max_tokens: 1024,
        context_window: 0,
    }
}

/// 事件 sink：返回（事件列表, Emitter），Emitter 交给 run_agent_loop。
fn sink_and_emit() -> (Arc<Mutex<Vec<AgentEvent>>>, pipi_core::agent_loop::Emitter) {
    let sink: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink2 = sink.clone();
    let emit: pipi_core::agent_loop::Emitter = Arc::new(move |event: AgentEvent| {
        sink2.lock().unwrap().push(event);
    });
    (sink, emit)
}

fn test_config(addr: std::net::SocketAddr, retry: RetryPolicy) -> AgentLoopConfig {
    AgentLoopConfig {
        model: test_model(addr),
        provider: provider_for(Api::OpenAICompletions),
        tools: Arc::new(ToolRegistry::new(vec![])),
        tool_context: ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
            approver: None,
        },
        options: StreamOptions {
            api_key: Some("test-key".into()),
            temperature: None,
            max_tokens: Some(64),
            timeout_secs: 5,
            session_id: None,
        },
        retry,
        tool_execution: pipi_core::agent_loop::ToolExecutionMode::Sequential,
        steering: pipi_core::agent_loop::MessageQueue::new(),
        follow_up: pipi_core::agent_loop::MessageQueue::new(),
        before_tool_call: None,
        after_tool_call: None,
        transform_context: None,
    }
}

/// 429 带 `Retry-After: 1`：服务端要求的等待必须优先于退避基数（50ms）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retryable_429_is_resent_after_the_requested_delay() {
    let rate_limited = http_response(
        "429 Too Many Requests",
        &[("Content-Type", "application/json"), ("Retry-After", "1")],
        r#"{"error":{"message":"rate limited"}}"#,
    );
    let (addr, requests) = spawn_http_script(vec![rate_limited, {
        // 第二次请求返回正常 SSE（注意：这个 mock 直接用完整响应原文）
        http_response(
            "200 OK",
            &[("Content-Type", "text/event-stream")],
            &sse_text("恢复后的回复"),
        )
    }]);

    let config = test_config(
        addr,
        RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 50,
            max_delay_ms: 5_000,
        },
    );
    let (sink, emit) = sink_and_emit();

    let started = std::time::Instant::now();
    let messages = run_agent_loop(
        vec![Message::user_text("hi")],
        AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
        },
        config,
        emit,
        AbortSignal::new(),
    )
    .await;
    let elapsed = started.elapsed();

    assert_eq!(requests.load(Ordering::SeqCst), 2, "应重发一次");
    assert!(
        elapsed >= std::time::Duration::from_millis(900),
        "应等服务端要求的 1 秒（而不是 50ms 退避），实际 {elapsed:?}"
    );
    match messages.last().expect("至少一条消息") {
        Message::Assistant {
            content,
            stop_reason,
            error_message,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Stop, "{error_message:?}");
            assert!(matches!(&content[0], pipi_core::types::ContentBlock::Text { text } if text == "恢复后的回复"));
        }
        other => panic!("期望成功的助手消息，得到 {other:?}"),
    }
    let events = sink.lock().unwrap();
    let retries: Vec<(u32, u32)> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RetryStart {
                attempt,
                max_attempts,
                ..
            } => Some((*attempt, *max_attempts)),
            _ => None,
        })
        .collect();
    assert_eq!(retries, vec![(1, 3)], "应发出一次 retry_start");
}

/// 400 是终态：不重发，直接把错误交回。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_400_is_not_resent() {
    let bad_request = http_response(
        "400 Bad Request",
        &[("Content-Type", "application/json")],
        r#"{"error":{"message":"invalid_request_error: context length exceeded"}}"#,
    );
    let (addr, requests) = spawn_http_script(vec![bad_request, {
        http_response(
            "200 OK",
            &[("Content-Type", "text/event-stream")],
            &sse_text("不该走到这里"),
        )
    }]);

    let config = test_config(addr, RetryPolicy::default());
    let (sink, emit) = sink_and_emit();

    let messages = run_agent_loop(
        vec![Message::user_text("hi")],
        AgentContext {
            system_prompt: String::new(),
            messages: Vec::new(),
        },
        config,
        emit,
        AbortSignal::new(),
    )
    .await;

    assert_eq!(requests.load(Ordering::SeqCst), 1, "终态错误不应重发");
    match messages.last().expect("至少一条消息") {
        Message::Assistant {
            stop_reason,
            error_message,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            let message = error_message.clone().unwrap_or_default();
            assert!(message.contains("400"), "{message}");
        }
        other => panic!("期望错误消息，得到 {other:?}"),
    }
    assert!(
        !sink
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, AgentEvent::RetryStart { .. })),
        "终态错误不该有 retry_start"
    );
}
