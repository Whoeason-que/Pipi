//! provider 请求时限：挂死的连接必须变成可见错误（`StreamEvent::Error`），
//! 而不是让会话无限期停在「运行中」。
//!
//! 覆盖两种形态：
//! 1. 端点接受连接但永不响应 —— 「首个响应」超时；
//! 2. 端点先发一个内容分片再静默 —— 「流中无增量」超时。

use std::io::{Read, Write};
use std::time::Duration;

use pipi_core::provider::provider_for;
use pipi_core::types::{AbortSignal, Api, Context, Message, Model, StreamEvent, StreamOptions};

/// 读掉一个 HTTP 请求（丢弃内容，保证响应前请求体已读完）。
fn drain_request(stream: &mut std::net::TcpStream) {
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
    let mut body = vec![0u8; content_length];
    let _ = stream.read_exact(&mut body);
}

fn bind_endpoint() -> (std::net::TcpListener, std::net::SocketAddr) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

/// 接受连接但永不响应。
fn spawn_silent_endpoint() -> std::net::SocketAddr {
    let (listener, addr) = bind_endpoint();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    addr
}

/// 先回一个内容分片，然后保持静默（不再发送任何数据）。
fn spawn_stalling_endpoint() -> std::net::SocketAddr {
    let (listener, addr) = bind_endpoint();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for mut stream in listener.incoming().flatten() {
            drain_request(&mut stream);
            let chunk = r#"{"id":"c1","object":"chat.completion.chunk","model":"mock","choices":[{"index":0,"delta":{"content":"开头"},"finish_reason":null}]}"#;
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
            let _ = stream.write_all(format!("data: {chunk}\n\n").as_bytes());
            let _ = stream.flush();
            held.push(stream);
        }
    });
    addr
}

/// 消费一次 provider 流；返回（是否收到过文本分片, 错误信息）。
async fn drain_stream(addr: std::net::SocketAddr, timeout_secs: u64) -> (bool, Option<String>) {
    let model = Model {
        id: "mock".into(),
        name: "Mock".into(),
        api: Api::OpenAICompletions,
        base_url: format!("http://{addr}/v1"),
        max_tokens: 64,
        context_window: 0,
    };
    let context = Context {
        system_prompt: None,
        messages: vec![Message::user_text("hi")],
        tools: Vec::new(),
    };
    let options = StreamOptions {
        api_key: Some("test-key".into()),
        temperature: None,
        max_tokens: Some(16),
        timeout_secs,
        session_id: None,
    };
    let mut rx = provider_for(Api::OpenAICompletions)
        .stream(&model, &context, &options, AbortSignal::new())
        .await;

    let mut saw_text = false;
    let mut error = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { .. } => saw_text = true,
            StreamEvent::Error { message } => {
                error = Some(message);
                break;
            }
            _ => {}
        }
    }
    (saw_text, error)
}

/// 外层兜底：实现若真的挂死，测试会在 8 秒内失败而不是无限等待。
async fn drain_with_deadline(
    addr: std::net::SocketAddr,
    timeout_secs: u64,
) -> (bool, Option<String>) {
    tokio::time::timeout(Duration::from_secs(8), drain_stream(addr, timeout_secs))
        .await
        .expect("请求时限未生效：provider 流没有产生任何事件也没有结束")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_endpoint_times_out_with_visible_error() {
    let addr = spawn_silent_endpoint();
    let (saw_text, error) = drain_with_deadline(addr, 1).await;
    assert!(!saw_text);
    let message = error.expect("挂死的端点必须产生可见错误，而不是无限等待");
    assert!(message.contains("超时"), "错误信息应说明超时：{message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_stream_times_out_after_partial_output() {
    let addr = spawn_stalling_endpoint();
    let (saw_text, error) = drain_with_deadline(addr, 1).await;
    assert!(saw_text, "首个分片应当已经送达");
    let message = error.expect("流中静默必须产生可见错误");
    assert!(message.contains("超时"), "错误信息应说明超时：{message}");
}
