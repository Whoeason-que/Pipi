//! 真进程端到端：握手、列工具、调用、错误、关闭。
//!
//! 用同一个 crate 的 `mock_mcp_server` 二进制（`CARGO_BIN_EXE_` 由 Cargo 注入），
//! 不需要任何外部依赖或网络。

use std::time::Duration;

use pipi_mcp::{Command, ConnectOptions, ContentPart, McpClient, McpError};

fn options() -> ConnectOptions {
    ConnectOptions {
        initialize_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
        ..ConnectOptions::default()
    }
}

async fn connect(mode: &str) -> McpClient {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mock_mcp_server"));
    if !mode.is_empty() {
        command.arg(mode);
    }
    McpClient::connect(command, options())
        .await
        .expect("mock server should connect")
}

#[tokio::test]
async fn handshake_reports_server_identity() {
    let client = connect("").await;
    let info = client.server_info();
    assert_eq!(info.name.as_deref(), Some("mock-mcp"));
    assert_eq!(info.version.as_deref(), Some("0.1.0"));
    assert_eq!(info.protocol_version, "2025-06-18");
    assert!(!client.is_closed());
    client.shutdown().await;
}

#[tokio::test]
async fn tools_carry_schema_and_description() {
    let client = connect("").await;
    let tools = client.tools().await.expect("tools/list should succeed");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["echo", "fail", "image", "slow", "grow"]);
    let echo = &tools[0];
    assert_eq!(echo.description.as_deref(), Some("Echo the message back"));
    assert_eq!(echo.input_schema["required"][0], "message");
    client.shutdown().await;
}

#[tokio::test]
async fn call_tool_maps_text_image_and_error_results() {
    let client = connect("").await;

    let echo = client
        .call_tool("echo", serde_json::json!({ "message": "hi" }))
        .await
        .expect("echo should succeed");
    assert!(!echo.is_error);
    assert!(matches!(&echo.content[0], ContentPart::Text(text) if text == "echo: hi"));

    let image = client
        .call_tool("image", serde_json::json!({}))
        .await
        .expect("image should succeed");
    assert!(matches!(
        &image.content[0],
        ContentPart::Image { data, mime_type } if data == "aGVsbG8=" && mime_type == "image/png"
    ));

    let failure = client
        .call_tool("fail", serde_json::json!({}))
        .await
        .expect("a tool-level failure is still a successful call");
    assert!(failure.is_error);

    client.shutdown().await;
}

#[tokio::test]
async fn rpc_errors_surface_with_code_and_message() {
    let client = connect("").await;
    let error = client
        .call_tool("missing", serde_json::json!({}))
        .await
        .expect_err("unknown tool should be an RPC error");
    match error {
        McpError::Rpc { code, message } => {
            assert_eq!(code, -32602);
            assert!(message.contains("unknown tool"));
        }
        other => panic!("expected Rpc error, got {other:?}"),
    }
    client.shutdown().await;
}

#[tokio::test]
async fn slow_calls_time_out_without_killing_the_connection() {
    let client = connect("").await;
    let error = client
        .call_tool_with_timeout("slow", serde_json::json!({}), Duration::from_millis(150))
        .await
        .expect_err("slow call should time out");
    assert!(matches!(error, McpError::Timeout { .. }));
    // 超时只影响那一次调用：连接仍可复用。
    let echo = client
        .call_tool("echo", serde_json::json!({ "message": "after" }))
        .await
        .expect("connection should still work");
    assert!(matches!(&echo.content[0], ContentPart::Text(text) if text == "echo: after"));
    client.shutdown().await;
}

#[tokio::test]
async fn server_exit_closes_pending_requests() {
    let client = connect("--die-after-init").await;
    let error = client
        .tools()
        .await
        .expect_err("server is gone, request should fail as closed");
    assert!(matches!(error, McpError::Closed));
    assert!(client.is_closed());
    client.shutdown().await;
}

#[tokio::test]
async fn empty_tool_list_is_not_an_error() {
    let client = connect("--no-tools").await;
    assert!(
        client
            .tools()
            .await
            .expect("empty list is valid")
            .is_empty()
    );
    client.shutdown().await;
}

#[tokio::test]
async fn cancellation_drops_the_wait_and_keeps_the_connection() {
    use tokio_util::sync::CancellationToken;

    let client = connect("").await;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let error = client
        .call_tool_cancellable(
            "slow",
            serde_json::json!({}),
            Duration::from_secs(5),
            Some(&token),
        )
        .await
        .expect_err("cancelled call should not return a result");
    assert!(matches!(error, McpError::Cancelled));

    // 服务器还在睡那 2 秒；睡完后连接照常可用（迟到的响应被丢弃）。
    let echo = client
        .call_tool("echo", serde_json::json!({ "message": "alive" }))
        .await
        .expect("connection should survive cancellation");
    assert!(matches!(&echo.content[0], ContentPart::Text(text) if text == "echo: alive"));
    client.shutdown().await;
}

#[tokio::test]
async fn tools_changed_notification_bumps_the_watch_version() {
    let client = connect("").await;
    let mut watch = client.watch_tools();
    assert_eq!(client.tools().await.expect("list").len(), 5);
    client
        .call_tool("grow", serde_json::json!({}))
        .await
        .expect("grow should succeed");
    tokio::time::timeout(Duration::from_secs(2), watch.changed())
        .await
        .expect("list_changed should be observed")
        .expect("watch channel is alive");
    let tools = client.tools().await.expect("relist");
    assert!(tools.iter().any(|tool| tool.name == "echo2"));
    client.shutdown().await;
}
