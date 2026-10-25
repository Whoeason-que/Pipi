//! 真实 rig HTTP 请求必须携带思考参数；默认绑定不能偷偷开启思考。
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use pipi_protocol::{
    AbortSignal, Api, Context, Message, Model, ReasoningEffort, StreamEvent, StreamOptions,
};
use serde_json::{Value, json};

async fn capture(api: Api, id: &str, effort: Option<ReasoningEffort>) -> Value {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "No HTTP request arrived");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("Cannot accept request: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
        let length: usize = header
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .expect("Content-Length")
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        let response = r#"{"error":{"type":"invalid_request_error","message":"request captured"}}"#;
        write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        serde_json::from_slice::<Value>(&body).unwrap()
    });
    let model = Model {
        id: id.into(),
        name: id.into(),
        api,
        base_url: format!("http://{addr}/v1"),
        max_tokens: 32768,
        context_window: 200000,
        reasoning_effort: effort,
    };
    let context = Context {
        messages: vec![Message::user_text("hi")],
        ..Default::default()
    };
    let options = StreamOptions {
        api_key: Some("test-key".into()),
        max_tokens: effort.is_none().then_some(32768),
        timeout_secs: 5,
        ..Default::default()
    };
    let mut events = pipi_provider::provider_for()
        .stream(&model, &context, &options, AbortSignal::new())
        .await;
    let mut rejected = false;
    while let Some(event) = events.recv().await {
        if let StreamEvent::Error { message, .. } = event {
            assert!(message.contains("request captured"), "{message}");
            rejected = true;
        }
    }
    assert!(rejected, "The capture endpoint must terminate the request");
    server.join().unwrap()
}

#[tokio::test]
async fn reasoning_survives_rig_wire_encoding_for_both_protocols() {
    for api in [Api::OpenAICompletions, Api::AnthropicMessages] {
        let body = capture(api, "test-model", None).await;
        for key in ["reasoning_effort", "thinking", "output_config"] {
            assert!(body.get(key).is_none(), "Default must omit {key}: {body}");
        }
    }
    let body = capture(
        Api::OpenAICompletions,
        "test-model",
        Some(ReasoningEffort::High),
    )
    .await;
    assert_eq!(body["reasoning_effort"], "high");
    assert!(body.get("thinking").is_none());
    let body = capture(
        Api::AnthropicMessages,
        "claude-sonnet-4-6",
        Some(ReasoningEffort::Medium),
    )
    .await;
    assert_eq!(body["thinking"], json!({"type":"adaptive"}));
    assert_eq!(body["output_config"]["effort"], "medium");
    assert!(body.get("reasoning_effort").is_none());
    let body = capture(
        Api::AnthropicMessages,
        "claude-sonnet-4-5",
        Some(ReasoningEffort::High),
    )
    .await;
    assert_eq!(
        body["thinking"],
        json!({"type":"enabled", "budget_tokens":16384})
    );
    assert_eq!(body["max_tokens"], 32768);
    let body = capture(
        Api::AnthropicMessages,
        "claude-sonnet-4-6",
        Some(ReasoningEffort::None),
    )
    .await;
    assert_eq!(body["thinking"], json!({"type":"disabled"}));
}
