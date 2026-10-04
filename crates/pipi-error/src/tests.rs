use super::*;

#[test]
fn error_codes_use_stable_snake_case() {
    assert_eq!(
        serde_json::to_string(&ErrorCode::PermissionDenied).unwrap(),
        "\"permission_denied\""
    );
}

#[test]
fn retry_hint_describes_but_does_not_decide_retrying() {
    let error = PipiError::retryable(
        ErrorCode::RateLimited,
        "请求过于频繁，请稍后重试",
        RetryHint::after(2_000),
    );
    assert_eq!(error.code(), ErrorCode::RateLimited);
    assert_eq!(error.user_message(), "请求过于频繁，请稍后重试");
    assert_eq!(error.retry_hint().unwrap().after_ms, Some(2_000));
}
