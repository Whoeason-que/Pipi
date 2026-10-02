//! rig 传输错误的分类。
//!
//! 这里只把底层失败归一为「可重试 / 终态」和服务端等待提示；重发次数、退避和
//! 用户中止语义留给 agent loop 的重试策略层，防止双层重试。

use pipi_error::RetryHint;
use rig::ProviderError;
use rig::http_client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryVerdict {
    Retryable,
    Terminal,
}

/// 把 rig 的结构化错误分类。只有 rig 无法结构化的两个字符串变体使用保守的
/// 特征词匹配；无法确认的错误一律终态。
///
/// rig 0.43 的错误边界：带状态的失败统一是 `ProviderResponse`（传输层不再抛
/// 状态码），无状态的传输失败是 `Http`，流被截断是 `Truncated`。
pub fn classify_rig_error(err: &ProviderError) -> (RetryVerdict, Option<RetryHint>) {
    match err {
        ProviderError::ProviderResponse(response) => classify_response(response),
        ProviderError::InvalidAuthentication(response) => classify_response(response),
        ProviderError::Http(http_error) => match &**http_error {
            http_client::Error::InvalidStatusCodeWithDetails {
                status,
                body,
                headers,
            } => classify_status(
                Some(status.as_u16()),
                retry_after_from_headers(Some(headers)),
                Some(body),
            ),
            http_client::Error::StreamEnded | http_client::Error::Instance(_) => {
                (RetryVerdict::Retryable, Some(RetryHint::plain()))
            }
            _ => (RetryVerdict::Terminal, None),
        },
        // 流在结束帧之前断了（干净 EOF、代理掐断）：可重试
        ProviderError::Truncated => (RetryVerdict::Retryable, Some(RetryHint::plain())),
        // 中继过来的报告带着它自己的判定
        ProviderError::Relayed(report) if report.retryable => {
            (RetryVerdict::Retryable, Some(RetryHint::plain()))
        }
        ProviderError::Provider(message) if transport_flavored(message) => {
            (RetryVerdict::Retryable, Some(RetryHint::plain()))
        }
        ProviderError::Response(message) if stream_truncation_flavored(message) => {
            (RetryVerdict::Retryable, Some(RetryHint::plain()))
        }
        _ => (RetryVerdict::Terminal, None),
    }
}

fn classify_response(
    response: &rig::ProviderResponseError,
) -> (RetryVerdict, Option<RetryHint>) {
    classify_status(
        response.status.map(|status| status.as_u16()),
        retry_after_from_headers(response.headers.as_ref()),
        Some(&response.body),
    )
}

/// 按 HTTP 状态码分类（`None` = 捕获不到状态码的传输失败）。
///
/// 408 / 409 / 429 / 5xx 可重试，其他状态 fail closed；429 的永久配额耗尽
/// 例外为终态。服务端给出的 `Retry-After` 必须原样保留给重试层。
pub fn classify_status(
    status: Option<u16>,
    hint: Option<RetryHint>,
    body: Option<&str>,
) -> (RetryVerdict, Option<RetryHint>) {
    let Some(status) = status else {
        return (RetryVerdict::Retryable, Some(RetryHint::plain()));
    };
    match status {
        408 | 409 => (
            RetryVerdict::Retryable,
            Some(hint.unwrap_or_else(RetryHint::plain)),
        ),
        429 if body.is_some_and(quota_exhausted) => (RetryVerdict::Terminal, None),
        429 | 500..=599 => (
            RetryVerdict::Retryable,
            Some(hint.unwrap_or_else(RetryHint::plain)),
        ),
        _ => (RetryVerdict::Terminal, None),
    }
}

fn retry_after_from_headers(headers: Option<&http::HeaderMap>) -> Option<RetryHint> {
    let value = headers?.get(http::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds: u64 = value.trim().parse().ok()?;
    Some(RetryHint::after(seconds.saturating_mul(1_000)))
}

fn quota_exhausted(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "insufficient_quota",
        "quota exceeded",
        "quota_exceeded",
        "exceeded your current quota",
        "billing",
        "credit balance",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

fn transport_flavored(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "error sending request",
        "connection",
        "connect",
        "reset",
        "broken pipe",
        "timed out",
        "timeout",
        "dns",
        "eof",
        "stream ended",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

fn stream_truncation_flavored(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "malformed json input",
        "stream ended",
        "stream closed",
        "ended before",
        "incomplete",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_preserved_for_every_retryable_status() {
        for status in [408, 409, 429, 500, 503, 504] {
            let (verdict, hint) =
                classify_status(Some(status), Some(RetryHint::after(2_000)), None);
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
}
