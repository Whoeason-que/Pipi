use super::*;

#[test]
fn message_wire_shape_keeps_pi_tags() {
    let message = Message::ToolResult {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        content: vec![ToolResultContent::Text { text: "ok".into() }],
        is_error: false,
        details: None,
        timestamp: 42,
    };
    let json = serde_json::to_value(&message).unwrap();
    assert_eq!(json["role"], "toolResult");
    assert_eq!(json["toolCallId"], "call-1");
    assert_eq!(json["content"][0]["type"], "text");
}

#[test]
fn error_envelope_exposes_only_safe_error_fields() {
    let error = PipiError::retryable(
        ErrorCode::RateLimited,
        "请求过于频繁，请稍后重试",
        pipi_error::RetryHint::after(1_000),
    );
    let envelope = ErrorEnvelope::from(&error);
    assert_eq!(envelope.code, ErrorCode::RateLimited);
    assert_eq!(envelope.retry_after_ms, Some(1_000));
}

#[test]
fn abort_signal_can_be_reused_for_the_next_turn() {
    let signal = AbortSignal::new();
    signal.abort();
    assert!(signal.is_aborted());
    signal.reset();
    assert!(!signal.is_aborted());
}

#[tokio::test]
async fn wait_aborted_wakes_up_on_abort() {
    let signal = AbortSignal::new();
    let waiter = signal.clone();
    let task = tokio::spawn(async move { waiter.wait_aborted().await });
    // 给等待方一个进入 await 的机会，再中止：旧实现要等下一次 50ms 轮询才醒
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    signal.abort();
    tokio::time::timeout(std::time::Duration::from_millis(200), task)
        .await
        .expect("abort 后等待方应立刻被唤醒")
        .expect("等待任务不应 panic");
}

#[tokio::test]
async fn wait_aborted_returns_immediately_when_already_aborted() {
    let signal = AbortSignal::new();
    signal.abort();
    // 已经中止的信号不应再等一个轮询周期
    tokio::time::timeout(std::time::Duration::from_millis(20), signal.wait_aborted())
        .await
        .expect("已中止的等待应立刻返回");
}

#[tokio::test]
async fn reset_makes_the_next_turn_abortable_again() {
    let signal = AbortSignal::new();
    signal.abort();
    signal.reset();
    let waiter = signal.clone();
    let task = tokio::spawn(async move { waiter.wait_aborted().await });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    signal.abort();
    tokio::time::timeout(std::time::Duration::from_millis(200), task)
        .await
        .expect("reset 之后的新一轮仍应能中止")
        .expect("等待任务不应 panic");
}
