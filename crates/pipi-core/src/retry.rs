//! 请求重试：错误分类与退避策略。
//!
//! 分工（对齐上游做法，见 README「请求重试」）：
//!
//! - **provider 层只做分类**：把 rig 的结构化错误映射成「可重试 / 终态」，并把
//!   服务端要求的等待时间（`Retry-After`）带进 [`StreamError`]，自己不重试；
//! - **agent_loop 层做重试**：判断这一轮能不能重放（是否已经向用户输出过正文）、
//!   算退避、与 abort 竞速。
//!
//! 重试只在 agent_loop 一处发生 —— 上游 pi/opencode 都在 provider 与会话两层
//! 各做一次重试，那会造成重试放大；codex 的分层（transport 分类 + 流层重试）
//! 才是我们要的形状。
//!
//! **用户中止永远不是可重试错误**：`AbortSignal` 被置位时直接收尾，不进这里。

use std::time::Duration;

use rig::completion::CompletionError;
use rig::http_client;

/// 服务端要求的等待提示（来自 `Retry-After` 一类头部）。
///
/// `after_ms` 为 `None` 只表示「可重试但没有明确等待要求」，此时按退避序列走。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryHint {
    pub after_ms: Option<u64>,
    /// 这次失败是「无进展超时」：每次尝试都要花掉整个 timeout 预算（默认 300s），
    /// 所以最多额外重试 1 次 —— 三次就是 15 分钟，用户会以为卡死。
    pub idle_timeout: bool,
}

impl RetryHint {
    pub const fn plain() -> Self {
        RetryHint {
            after_ms: None,
            idle_timeout: false,
        }
    }

    pub const fn after(after_ms: u64) -> Self {
        RetryHint {
            after_ms: Some(after_ms),
            idle_timeout: false,
        }
    }

    /// 无进展超时（受「最多额外重试 1 次」限制）。
    pub const fn timeout() -> Self {
        RetryHint {
            after_ms: None,
            idle_timeout: true,
        }
    }
}

/// 进入事件流的错误：用户可读的消息 + 重试提示。
///
/// `retry: None` 表示终态（重试没有意义：请求本身不合法、鉴权失败、
/// 上下文超限、配额耗尽……）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamError {
    pub message: String,
    pub retry: Option<RetryHint>,
}

impl StreamError {
    /// 终态错误。
    pub fn terminal(message: impl Into<String>) -> Self {
        StreamError {
            message: message.into(),
            retry: None,
        }
    }

    /// 可重试，没有指定等待。
    pub fn retryable(message: impl Into<String>) -> Self {
        StreamError {
            message: message.into(),
            retry: Some(RetryHint::plain()),
        }
    }

    /// 可重试，并可带服务端要求的等待。
    pub fn retryable_with(message: impl Into<String>, hint: Option<RetryHint>) -> Self {
        StreamError {
            message: message.into(),
            retry: Some(hint.unwrap_or_else(RetryHint::plain)),
        }
    }
}

impl From<String> for StreamError {
    fn from(message: String) -> Self {
        StreamError::terminal(message)
    }
}

/// 单条错误的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryVerdict {
    Retryable,
    Terminal,
}

/// 重试策略（全局设置 `retry` 段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 总尝试次数（含首次）：1 表示不重试。
    pub max_attempts: u32,
    /// 退避基数：第 n 次失败后等待 `base * 2^(n-1)`。
    pub base_delay_ms: u64,
    /// 退避上限；服务端要求的等待超过它时不再重试（见 [`DelayDecision::TooLong`]）。
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        }
    }
}

impl RetryPolicy {
    /// 关掉重试（测试与「只发一次」场景用）。
    pub const NONE: RetryPolicy = RetryPolicy {
        max_attempts: 1,
        base_delay_ms: 0,
        max_delay_ms: 0,
    };

    /// 已经失败了 `failures` 次之后，还允许再试吗（调用点在失败之后，`failures ≥ 1`）。
    ///
    /// `max_attempts = 3` 表示「总共最多发 3 次请求」= 最多重试 2 次。
    pub fn allows_more(&self, failures: u32) -> bool {
        failures < self.max_attempts
    }
}

/// 一次失败的处置：重试（等多久）或放弃（为什么）。
///
/// 把「策略上限」「超时类额外重试 1 次」「服务端要求的等待超过上限」三条规则
/// 收在一个纯函数里，便于表驱动测试与在注释里对照上游。
pub fn retry_decision(
    policy: &RetryPolicy,
    failures: u32,
    hint: Option<RetryHint>,
) -> RetryDecision {
    if !policy.allows_more(failures) {
        return RetryDecision::GiveUp(GiveUpReason::PolicyExhausted);
    }
    if hint.is_some_and(|hint| hint.idle_timeout) && failures >= 2 {
        // 第一次超时还允许再试一次；第二次超时不再试（每次尝试都是整个 timeout 预算）
        return RetryDecision::GiveUp(GiveUpReason::TimeoutBudget);
    }
    match hint.and_then(|hint| hint.after_ms) {
        // 服务端要求等更久：不再等，直接收尾（并在错误里说明等待时长）
        Some(after_ms) if after_ms > policy.max_delay_ms => {
            RetryDecision::GiveUp(GiveUpReason::ServerDelayTooLong(after_ms))
        }
        Some(after_ms) => RetryDecision::Retry {
            delay: Duration::from_millis(after_ms),
        },
        None => {
            let exponent = failures.saturating_sub(1).min(16);
            let raw = policy
                .base_delay_ms
                .saturating_mul(1u64 << exponent)
                .min(policy.max_delay_ms);
            RetryDecision::Retry {
                delay: with_jitter(Duration::from_millis(raw), jitter_sample()),
            }
        }
    }
}

/// [`retry_decision`] 的结论。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetryDecision {
    /// 等这么久再发一次。
    Retry { delay: Duration },
    /// 不再重试，按终态收尾。
    GiveUp(GiveUpReason),
}

/// 放弃重试的原因（用于最终错误文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveUpReason {
    /// 用完策略允许的尝试次数。
    PolicyExhausted,
    /// 超时类的额外重试预算用尽（每次尝试都很贵）。
    TimeoutBudget,
    /// 服务端要求等待的时间超过策略上限。
    ServerDelayTooLong(u64),
}

/// 抖动：`delay` 收敛到 [0.75, 1.0] × delay。`sample` 取 [0, 1)。
///
/// 纯函数，便于测试；随机源见 [`jitter_sample`]。
pub fn with_jitter(delay: Duration, sample: f64) -> Duration {
    let factor = 1.0 - 0.25 * sample.clamp(0.0, 1.0);
    delay.mul_f64(factor)
}

/// 取一个 [0, 1) 的采样（不需要引入 rand：`RandomState` 由进程熵播种）。
pub fn jitter_sample() -> f64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    (hasher.finish() % 1_000_000) as f64 / 1_000_000.0
}

/// 把 rig 的错误分类成「可重试 / 终态」。
///
/// rig 的错误是结构化的（状态码、响应头、原始 body 都在），所以这里尽量按结构
/// 判定；只有 `ProviderError(String)` 与 `ResponseError(String)` 两类字符串错误
/// 需要按特征词匹配 —— 这是全仓唯一允许的「按错误文本判定」的地方。
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
            http_client::Error::InvalidStatusCodeWithDetails { status, body, headers } => {
                classify_status(
                    Some(status.as_u16()),
                    retry_after_from_headers(Some(headers)),
                    Some(body),
                )
            }
            // 流在结束帧之前断开：可重试
            http_client::Error::StreamEnded => {
                (RetryVerdict::Retryable, Some(RetryHint::plain()))
            }
            // 传输层失败（reqwest 发送失败 / 连接重置 / DNS）：可重试
            http_client::Error::Instance(_) => {
                (RetryVerdict::Retryable, Some(RetryHint::plain()))
            }
            // 本地协议 / 头部构造问题：重试无用
            _ => (RetryVerdict::Terminal, None),
        },
        CompletionError::ProviderError(message) => {
            if transport_flavored(message) {
                (RetryVerdict::Retryable, Some(RetryHint::plain()))
            } else {
                (RetryVerdict::Terminal, None)
            }
        }
        CompletionError::ResponseError(message) => {
            if stream_truncation_flavored(message) {
                (RetryVerdict::Retryable, Some(RetryHint::plain()))
            } else {
                (RetryVerdict::Terminal, None)
            }
        }
        // 请求构造 / 序列化 / URL 问题：重试同样会失败
        CompletionError::RequestError(_)
        | CompletionError::JsonError(_)
        | CompletionError::UrlError(_) => (RetryVerdict::Terminal, None),
        #[allow(unreachable_patterns)]
        _ => (RetryVerdict::Terminal, None),
    }
}

/// 按 HTTP 状态码分类（`None` = 捕获不到状态码的传输失败）。
///
/// | 状态 | 判定 |
/// | --- | --- |
/// | 408 / 409 / 429 / 5xx | 可重试 |
/// | 400 / 401 / 403 / 404 / 413 / 422 | 终态（请求不合法、鉴权、太大） |
/// | 其余 4xx | 终态 |
pub fn classify_status(
    status: Option<u16>,
    hint: Option<RetryHint>,
    body: Option<&str>,
) -> (RetryVerdict, Option<RetryHint>) {
    let Some(status) = status else {
        // 没有状态码 = 连接层失败（发送失败 / 连接重置 / DNS）
        return (RetryVerdict::Retryable, Some(RetryHint::plain()));
    };
    match status {
        408 | 409 => (RetryVerdict::Retryable, Some(RetryHint::plain())),
        429 => {
            // 429 里区分「限额已用尽」：等多久都不会好，属于终态
            if body.is_some_and(quota_exhausted) {
                return (RetryVerdict::Terminal, None);
            }
            (RetryVerdict::Retryable, Some(hint.unwrap_or_else(RetryHint::plain)))
        }
        500..=599 => (RetryVerdict::Retryable, Some(RetryHint::plain())),
        _ => (RetryVerdict::Terminal, None),
    }
}

/// `Retry-After`（秒）。HTTP-date 形式不回填 —— 解析日期需要额外依赖，
/// 认不出时退回退避序列（比误判安全）。
fn retry_after_from_headers(headers: Option<&http::HeaderMap>) -> Option<RetryHint> {
    let value = headers?.get(http::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds: u64 = value.trim().parse().ok()?;
    Some(RetryHint::after(seconds.saturating_mul(1_000)))
}

/// 配额 / 计费类终态词（只在 429 的响应体里找）。
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

/// 传输层失败的特征词（`ProviderError` 一类字符串错误里）。
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

/// 流在工具调用参数输出到一半时被截断（rig 的 `UnparseableToolInput`），
/// 或流在终态事件之前结束。这类错误重试通常能恢复。
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

    fn verdict(err: &CompletionError) -> (RetryVerdict, Option<RetryHint>) {
        classify_rig_error(err)
    }

    #[test]
    fn status_table() {
        for (status, expected) in [
            (408u16, RetryVerdict::Retryable),
            (409, RetryVerdict::Retryable),
            (429, RetryVerdict::Retryable),
            (500, RetryVerdict::Retryable),
            (502, RetryVerdict::Retryable),
            (503, RetryVerdict::Retryable),
            (504, RetryVerdict::Retryable),
            (400, RetryVerdict::Terminal),
            (401, RetryVerdict::Terminal),
            (403, RetryVerdict::Terminal),
            (404, RetryVerdict::Terminal),
            (413, RetryVerdict::Terminal),
            (422, RetryVerdict::Terminal),
            (418, RetryVerdict::Terminal),
        ] {
            let (got, _) = classify_status(Some(status), None, None);
            assert_eq!(got, expected, "status {status}");
        }
        // 没有状态码（连接层失败）→ 可重试
        assert_eq!(classify_status(None, None, None).0, RetryVerdict::Retryable);
    }

    #[test]
    fn quota_exhaustion_429_is_terminal() {
        let body = r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#;
        assert_eq!(
            classify_status(Some(429), None, Some(body)).0,
            RetryVerdict::Terminal
        );
        // 普通限流仍然可重试，并带上 Retry-After
        let (verdict, hint) = classify_status(Some(429), Some(RetryHint::after(2_000)), Some("slow down"));
        assert_eq!(verdict, RetryVerdict::Retryable);
        assert_eq!(hint.unwrap().after_ms, Some(2_000));
    }

    #[test]
    fn provider_response_error_uses_structured_status_and_headers() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::RETRY_AFTER, "3".parse().unwrap());
        let error = CompletionError::ProviderResponse(
            rig::ProviderResponseError::new(http::StatusCode::TOO_MANY_REQUESTS, "rate limited")
                .with_headers(Some(Box::new(headers))),
        );
        let (allowed, hint) = verdict(&error);
        assert_eq!(allowed, RetryVerdict::Retryable);
        assert_eq!(hint.unwrap().after_ms, Some(3_000));

        // 400 一律终态
        let bad = CompletionError::ProviderResponse(rig::ProviderResponseError::new(
            http::StatusCode::BAD_REQUEST,
            "invalid_request_error",
        ));
        assert_eq!(verdict(&bad).0, RetryVerdict::Terminal);
    }

    #[test]
    fn http_error_variants() {
        let details = CompletionError::HttpError(http_client::Error::InvalidStatusCodeWithDetails {
            status: http::StatusCode::BAD_GATEWAY,
            body: "bad gateway".into(),
            headers: Box::new(http::HeaderMap::new()),
        });
        assert_eq!(verdict(&details).0, RetryVerdict::Retryable);

        let ended = CompletionError::HttpError(http_client::Error::StreamEnded);
        assert_eq!(verdict(&ended).0, RetryVerdict::Retryable);

        let transport = CompletionError::HttpError(http_client::Error::Instance(Box::new(
            std::io::Error::other("error sending request for url"),
        )));
        assert_eq!(verdict(&transport).0, RetryVerdict::Retryable);

        let protocol = CompletionError::HttpError(http_client::Error::StreamEnded);
        assert_eq!(verdict(&protocol).0, RetryVerdict::Retryable);
    }

    #[test]
    fn string_errors_match_observed_failures() {
        // Q 会话实录：连接失败
        let offline = CompletionError::ProviderError(
            "Http client error: error sending request for url (https://opencode.ai/zen/go/v1/chat/completions)"
                .into(),
        );
        assert_eq!(verdict(&offline).0, RetryVerdict::Retryable);

        // Q 会话实录：工具调用参数被截断（rig UnparseableToolInput）
        let truncated = CompletionError::ResponseError(
            "tool call `edit` arrived with malformed JSON input: EOF while parsing a string at line 1 column 2559"
                .into(),
        );
        assert_eq!(verdict(&truncated).0, RetryVerdict::Retryable);

        // 其余字符串错误保持终态（保守）
        let other = CompletionError::ProviderError("model `x` not found".into());
        assert_eq!(verdict(&other).0, RetryVerdict::Terminal);
        let other = CompletionError::ResponseError("unexpected tool call id".into());
        assert_eq!(verdict(&other).0, RetryVerdict::Terminal);
    }

    #[test]
    fn local_errors_are_terminal() {
        let json = CompletionError::JsonError(serde_json::from_str::<i32>("nope").unwrap_err());
        assert_eq!(verdict(&json).0, RetryVerdict::Terminal);
        let request = CompletionError::RequestError(Box::new(std::io::Error::other("bad request")));
        assert_eq!(verdict(&request).0, RetryVerdict::Terminal);
        // `CompletionError::UrlError` 需要 url 的直接依赖才能构造，仓库里没有；
        // 它落在同一个终态分支上（见 classify_rig_error 的 match 臂）。
    }

    #[test]
    fn backoff_sequence_caps_and_jitters() {
        let policy = RetryPolicy::default();
        // 前两次失败 → 1s / 2s（抖动后落在 [0.75, 1.0]×）；第三次失败已无额度
        for failures in 1..=2u32 {
            let RetryDecision::Retry { delay } = retry_decision(&policy, failures, None) else {
                panic!("第 {failures} 次失败应重试");
            };
            let raw = policy.base_delay_ms * (1 << (failures - 1));
            assert!(delay.as_millis() >= (raw as f64 * 0.75) as u128);
            assert!(delay.as_millis() <= raw as u128);
        }
        // 上限：给足额度时，指数增长到封顶值就不再增长
        let generous = RetryPolicy {
            max_attempts: 100,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        };
        let RetryDecision::Retry { delay } = retry_decision(&generous, 20, None) else {
            panic!("给足额度时应重试");
        };
        assert!(delay.as_millis() <= generous.max_delay_ms as u128);
        assert!(
            delay.as_millis() >= (generous.max_delay_ms as f64 * 0.75) as u128,
            "长退避应停在上限附近：{delay:?}"
        );

        // 尝试次数门控：3 次尝试 = 最多重试 2 次
        assert!(policy.allows_more(1));
        assert!(policy.allows_more(2));
        assert!(!policy.allows_more(3));
        assert_eq!(
            retry_decision(&policy, 3, None),
            RetryDecision::GiveUp(GiveUpReason::PolicyExhausted)
        );
        assert_eq!(
            retry_decision(&RetryPolicy::NONE, 1, None),
            RetryDecision::GiveUp(GiveUpReason::PolicyExhausted)
        );
    }

    #[test]
    fn idle_timeout_gets_at_most_one_retry() {
        let policy = RetryPolicy::default();
        // 第一次超时 → 允许再试一次
        assert!(matches!(
            retry_decision(&policy, 1, Some(RetryHint::timeout())),
            RetryDecision::Retry { .. }
        ));
        // 第二次超时 → 即使策略还有额度也不再试（每次尝试都是整个 timeout 预算）
        assert_eq!(
            retry_decision(&policy, 2, Some(RetryHint::timeout())),
            RetryDecision::GiveUp(GiveUpReason::TimeoutBudget)
        );
        // 普通错误不受这条限制
        assert!(matches!(
            retry_decision(&policy, 2, Some(RetryHint::plain())),
            RetryDecision::Retry { .. }
        ));
    }

    #[test]
    fn server_requested_delay_wins_and_can_be_too_long() {
        let policy = RetryPolicy::default();
        assert_eq!(
            retry_decision(&policy, 1, Some(RetryHint::after(5_000))),
            RetryDecision::Retry {
                delay: Duration::from_millis(5_000)
            }
        );
        // 超出上限：不再等待，交回调用方按终态处理
        assert_eq!(
            retry_decision(&policy, 1, Some(RetryHint::after(600_000))),
            RetryDecision::GiveUp(GiveUpReason::ServerDelayTooLong(600_000))
        );
    }

    #[test]
    fn jitter_stays_in_range() {
        let delay = Duration::from_millis(1_000);
        assert_eq!(with_jitter(delay, 0.0), delay);
        assert_eq!(with_jitter(delay, 1.0), Duration::from_millis(750));
        let sample = jitter_sample();
        assert!((0.0..1.0).contains(&sample), "采样越界：{sample}");
    }
}
