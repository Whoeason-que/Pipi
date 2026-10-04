use super::*;

#[test]
fn retry_after_is_preserved_for_every_retryable_status() {
    for status in [408, 409, 429, 500, 503, 504] {
        let (verdict, hint) = classify_status(Some(status), Some(RetryHint::after(2_000)), None);
        assert_eq!(verdict, RetryVerdict::Retryable, "status {status}");
        assert_eq!(hint.unwrap().after_ms, Some(2_000), "status {status}");
    }
}

#[test]
fn quota_exhaustion_is_terminal_but_normal_rate_limit_retries() {
    assert_eq!(
        classify_status(
            Some(429),
            None,
            Some(r#"{"error":{"code":"insufficient_quota"}}"#),
        )
        .0,
        RetryVerdict::Terminal
    );
    assert_eq!(
        classify_status(Some(429), None, Some("slow down")).0,
        RetryVerdict::Retryable
    );
}

#[test]
fn structured_retry_after_is_extracted() {
    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::RETRY_AFTER, "3".parse().unwrap());
    let mut response =
        rig::ProviderResponseError::new(http::StatusCode::TOO_MANY_REQUESTS, "rate limited");
    response.headers = Some(headers);
    let error = ProviderError::ProviderResponse(response);
    assert_eq!(classify_rig_error(&error).1.unwrap().after_ms, Some(3_000));
}

#[test]
fn truncated_streams_are_retryable_but_completed_errors_are_terminal() {
    assert_eq!(
        classify_rig_error(&ProviderError::Truncated).0,
        RetryVerdict::Retryable
    );
    assert_eq!(
        classify_rig_error(&ProviderError::Response("bad request shape".into())).0,
        RetryVerdict::Terminal
    );
}

#[test]
fn relayed_reports_keep_their_own_verdict() {
    let mut report = rig::ErrorReport::from(&ProviderError::Provider("boom".into()));
    report.retryable = true;
    assert_eq!(
        classify_rig_error(&ProviderError::Relayed(Box::new(report))).0,
        RetryVerdict::Retryable
    );
}
