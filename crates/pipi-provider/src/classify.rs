//! rig 传输错误的分类。
//!
//! 这里只把底层失败归一为「可重试 / 终态」和服务端等待提示；重发次数、退避和
//! 用户中止语义留给 agent loop 的重试策略层，防止双层重试。

use pipi_error::RetryHint;
use rig::completion::CompletionError;
use rig::http_client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryVerdict {
    Retryable,
    Terminal,
}

/// 把 rig 的结构化错误分类。只有 rig 无法结构化的两个字符串变体使用保守的
/// 特征词匹配；无法确认的错误一律终态。
pub fn classify_rig_error(err: &CompletionError) -> (RetryVerdict, Option<RetryHint>) {
    match err {
        CompletionError::ProviderResponse(response) => classify_status(
            response.status.map(|status| status.as_u16()),
            retry_after_from_headers(response.headers.as_deref()),
            Some(&response.body),
        ),
        CompletionError::HttpError(http_error) => match http_error {
            http_client::Error::InvalidStatusCode(status) => {
                classify_status(Some(status.as_u16()), None, None)
            }
            http_client::Error::InvalidStatusCodeWithMessage(status, body) => {
                classify_status(Some(status.as_u16()), None, Some(body))
            }
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
        CompletionError::ProviderError(message) if transport_flavored(message) => {
            (RetryVerdict::Retryable, Some(RetryHint::plain()))
        }
        CompletionError::ResponseError(message) if stream_truncation_flavored(message) => {
            (RetryVerdict::Retryable, Some(RetryHint::plain()))
        }
        _ => (RetryVerdict::Terminal, None),
    }
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
        let error = CompletionError::ProviderResponse(
            rig::ProviderResponseError::new(http::StatusCode::TOO_MANY_REQUESTS, "rate limited")
                .with_headers(Some(Box::new(headers))),
        );
        assert_eq!(classify_rig_error(&error).1.unwrap().after_ms, Some(3_000));
    }
}
