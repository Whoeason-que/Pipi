//! 用量口径的端到端回归：真实 SSE 报文 → rig 解析 → Pipi 归一化 → 会话统计。
//!
//! 背景：OpenAI 兼容端点的 `prompt_tokens` **已包含** `prompt_tokens_details.cached_tokens`
//! （rig 原样透传进 `input_tokens`），Anthropic 的 `input_tokens` 则不含缓存。若不归一化，
//! `input` 与 `cache_read` 重叠，命中率恒为 50%、上下文占用翻倍。
//!
//! 这里的数字取自一次真实会话（deepseek 经 openai-completions 协议）：
//! prompt 610566 / cached 610432 / completion 165 —— 真实命中率 99.98%，而不是 50%。

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use pipi_core::provider::provider_for;
use pipi_core::stats::SessionStatsTracker;
use pipi_core::types::{
    AbortSignal, Api, Context, Message, Model, StreamEvent, StreamOptions, Usage,
};

const PROMPT_TOKENS: u64 = 610_566;
const CACHED_TOKENS: u64 = 610_432;
const COMPLETION_TOKENS: u64 = 165;
const TOTAL_TOKENS: u64 = 610_731;

/// 读掉一个 HTTP 请求，返回请求体（用于断言 rig 确实请求了用量）。
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
    let headers = String::from_utf8_lossy(&buffer).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    let _ = stream.read_exact(&mut body);
    String::from_utf8_lossy(&body).to_string()
}

/// OpenAI 兼容的流式端点：一个内容分片 + 结束分片 + 只带用量的收尾分片。
fn spawn_usage_endpoint() -> (std::net::SocketAddr, Arc<Mutex<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen_request = Arc::new(Mutex::new(String::new()));
    let captured = seen_request.clone();

    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let body = read_request(&mut stream);
            if let Ok(mut slot) = captured.lock() {
                *slot = body;
            }
            let frames = [
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{"role":"assistant","content":"好"},"finish_reason":null}]}"#.to_string(),
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_string(),
                format!(
                    r#"{{"id":"c1","object":"chat.completion.chunk","created":1,"model":"mock","choices":[],"usage":{{"prompt_tokens":{PROMPT_TOKENS},"completion_tokens":{COMPLETION_TOKENS},"total_tokens":{TOTAL_TOKENS},"prompt_tokens_details":{{"cached_tokens":{CACHED_TOKENS}}}}}}}"#
                ),
            ];
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
            for frame in frames {
                let _ = stream.write_all(format!("data: {frame}\n\n").as_bytes());
            }
            let _ = stream.write_all(b"data: [DONE]\n\n");
            let _ = stream.flush();
        }
    });
    (addr, seen_request)
}

async fn run_once(addr: std::net::SocketAddr) -> (Usage, Message) {
    let model = Model {
        id: "mock".into(),
        name: "Mock".into(),
        api: Api::OpenAICompletions,
        base_url: format!("http://{addr}/v1"),
        max_tokens: 64,
        context_window: 1_000_000,
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
        timeout_secs: 5,
        session_id: None,
    };
    let mut rx = provider_for(Api::OpenAICompletions)
        .stream(&model, &context, &options, AbortSignal::new())
        .await;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Done { usage, message, .. } => return (usage, *message),
            StreamEvent::Error { message, .. } => panic!("provider 报错：{message}"),
            _ => {}
        }
    }
    panic!("流结束但没有 Done 事件");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_cache_usage_is_normalized_to_pi_semantics() {
    let (addr, seen_request) = spawn_usage_endpoint();
    let (usage, message) = run_once(addr).await;

    // rig 必须请求用量分片，否则端点不会上报缓存命中
    let body = seen_request.lock().unwrap().clone();
    assert!(
        body.contains(r#""include_usage":true"#),
        "请求体没有要求 include_usage：{body}"
    );

    // 归一化：input 只计未命中部分，命中量不与它重叠
    assert_eq!(usage.input, PROMPT_TOKENS - CACHED_TOKENS);
    assert_eq!(usage.cache_read, CACHED_TOKENS);
    assert_eq!(usage.cache_write, 0);
    assert_eq!(usage.output, COMPLETION_TOKENS);
    assert_eq!(usage.prompt_tokens(), PROMPT_TOKENS);
    assert_eq!(usage.total_tokens, TOTAL_TOKENS);
    assert_eq!(usage.total_tokens, usage.total());

    // 消息上带的用量与 Done 事件一致（会话文件持久化的就是它）
    let Message::Assistant {
        usage: persisted, ..
    } = &message
    else {
        panic!("Done 应当携带 assistant 消息");
    };
    assert_eq!(*persisted, usage);

    // 会话统计：命中率与上下文占用都按提示词总量算
    let mut tracker = SessionStatsTracker::new(Some(1_000_000));
    tracker.record(&message);
    let stats = tracker.snapshot();
    let hit = stats.cache_hit_pct.expect("有缓存数据时必须给出命中率");
    assert!((hit - 99.98).abs() < 0.01, "命中率应为 99.98%，实际 {hit}");
    assert_eq!(stats.context_used, Some(PROMPT_TOKENS));
    assert_eq!(stats.cache_read, CACHED_TOKENS);
    assert_eq!(stats.input, PROMPT_TOKENS - CACHED_TOKENS);
    assert_eq!(stats.output, COMPLETION_TOKENS);
}
