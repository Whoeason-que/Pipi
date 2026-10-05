//! Pipi 的跨层错误语义。
//!
//! 这个 crate 只保存稳定、可安全暴露给宿主和前端的信息；底层 crate 自己的
//! `io` / HTTP / JSON source chain 仍留在其局部实现里，跨 crate 边界时再归一化。
//! 它不能依赖 Pipi 的其他 crate，避免错误类型反过来制造依赖环。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 可跨 IPC / Web 边界保持稳定的错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    PermissionDenied,
    Cancelled,
    Timeout,
    RateLimited,
    Provider,
    Storage,
    Internal,
}

/// 可重试错误附带的等待提示。
///
/// `after_ms` 来自服务端（例如 `Retry-After`）；`idle_timeout` 表示这次失败已
/// 消耗完整的无进展超时预算，重试层应执行更严格的次数限制。它只描述事实，
/// 不在这里做任何重发决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryHint {
    pub after_ms: Option<u64>,
    pub idle_timeout: bool,
}

impl RetryHint {
    pub const fn plain() -> Self {
        Self {
            after_ms: None,
            idle_timeout: false,
        }
    }

    pub const fn after(after_ms: u64) -> Self {
        Self {
            after_ms: Some(after_ms),
            idle_timeout: false,
        }
    }

    pub const fn timeout() -> Self {
        Self {
            after_ms: None,
            idle_timeout: true,
        }
    }
}

/// 已归一化的应用错误。
///
/// `message` 必须是可直接展示给用户的安全文案；不要把 token、HTTP 原始 body、
/// 绝对私密路径或底层 source 的 `Display` 直接放进这里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipiError {
    code: ErrorCode,
    message: String,
    retry: Option<RetryHint>,
}

impl PipiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry: None,
        }
    }

    pub fn retryable(code: ErrorCode, message: impl Into<String>, hint: RetryHint) -> Self {
        Self {
            code,
            message: message.into(),
            retry: Some(hint),
        }
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn user_message(&self) -> &str {
        &self.message
    }

    pub fn retry_hint(&self) -> Option<RetryHint> {
        self.retry
    }
}

impl fmt::Display for PipiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PipiError {}

#[cfg(test)]
mod tests {
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
}
