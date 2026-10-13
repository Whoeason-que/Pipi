//! 跨层错误语义：真正跨 IPC / Web 边界传播的错误形状只有重试提示。
//!
//! 底层 crate 自己的 `io` / HTTP / JSON source chain 仍留在其局部实现里，
//! 跨 crate 边界时归一化成这里的类型；这里不放任何重发决定。

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_hint_constructors_describe_but_do_not_decide_retrying() {
        assert_eq!(
            RetryHint::plain(),
            RetryHint {
                after_ms: None,
                idle_timeout: false,
            }
        );
        assert_eq!(RetryHint::after(2_000).after_ms, Some(2_000));
        assert!(RetryHint::timeout().idle_timeout);
    }

    #[test]
    fn retry_hint_keeps_its_camel_case_wire_shape() {
        let hint = serde_json::to_value(RetryHint::after(2_000)).unwrap();
        assert_eq!(
            hint,
            serde_json::json!({ "afterMs": 2_000, "idleTimeout": false })
        );
    }
}
