//! Agent loop 的重试决策与退避策略。
//!
//! provider 只负责把底层错误写成带 [`RetryHint`] 的流事件；这里独占「是否再发、
//! 等多久」的决策。这样用户中止、已输出正文以及超时预算都只在一个位置生效。

use std::time::Duration;

use pipi_error::RetryHint;

/// 重试策略（全局设置 `retry` 段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 总尝试次数（含首次）：1 表示不重试。
    pub max_attempts: u32,
    /// 退避基数：第 n 次失败后等待 `base * 2^(n-1)`。
    pub base_delay_ms: u64,
    /// 退避上限；服务端要求的等待超过它时不再重试。
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
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

    /// 已经失败了 `failures` 次之后，还允许再试吗。
    pub fn allows_more(&self, failures: u32) -> bool {
        failures < self.max_attempts
    }
}

/// 一次失败的处置：重试（等多久）或放弃（为什么）。
pub fn retry_decision(
    policy: &RetryPolicy,
    failures: u32,
    hint: Option<RetryHint>,
) -> RetryDecision {
    if !policy.allows_more(failures) {
        return RetryDecision::GiveUp(GiveUpReason::PolicyExhausted);
    }
    if hint.is_some_and(|hint| hint.idle_timeout) && failures >= 2 {
        // 无进展超时已经耗尽一次完整请求预算，只额外允许一次重试。
        return RetryDecision::GiveUp(GiveUpReason::TimeoutBudget);
    }
    match hint.and_then(|hint| hint.after_ms) {
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
    Retry { delay: Duration },
    GiveUp(GiveUpReason),
}

/// 放弃重试的原因（用于最终错误文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveUpReason {
    PolicyExhausted,
    TimeoutBudget,
    ServerDelayTooLong(u64),
}

/// 抖动：`delay` 收敛到 [0.75, 1.0] × delay。`sample` 取 [0, 1)。
pub fn with_jitter(delay: Duration, sample: f64) -> Duration {
    let factor = 1.0 - 0.25 * sample.clamp(0.0, 1.0);
    delay.mul_f64(factor)
}

/// 取一个 [0, 1) 的采样。用 `fastrand`（thread-local，无全局锁）而不是自己拿
/// `RandomState` 拼哈希：后者是拿哈希的雪崩性当随机数用，分布没有保证。
pub fn jitter_sample() -> f64 {
    fastrand::f64()
}

#[cfg(test)]
mod tests;
