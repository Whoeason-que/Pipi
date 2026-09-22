//! Agent loop 的重试决策与退避策略。
//!
//! provider 只负责把底层错误写成带 [`RetryHint`] 的流事件；这里独占「是否再发、
//! 等多久」的决策。这样用户中止、已输出正文以及超时预算都只在一个位置生效。

use std::time::Duration;

/// 服务端等待提示是跨 crate 的稳定错误元数据；兼容既有调用点从 `retry`
/// 重导出，实际定义在没有业务依赖的 `pipi-error`。
pub use pipi_error::RetryHint;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sequence_caps_and_jitters() {
        let policy = RetryPolicy::default();
        for failures in 1..=2u32 {
            let RetryDecision::Retry { delay } = retry_decision(&policy, failures, None) else {
                panic!("第 {failures} 次失败应重试");
            };
            let raw = policy.base_delay_ms * (1 << (failures - 1));
            assert!(delay.as_millis() >= (raw as f64 * 0.75) as u128);
            assert!(delay.as_millis() <= raw as u128);
        }

        let generous = RetryPolicy {
            max_attempts: 100,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        };
        let RetryDecision::Retry { delay } = retry_decision(&generous, 20, None) else {
            panic!("给足额度时应重试");
        };
        assert!(delay.as_millis() <= generous.max_delay_ms as u128);
        assert!(delay.as_millis() >= (generous.max_delay_ms as f64 * 0.75) as u128);
        assert!(!policy.allows_more(3));
        assert_eq!(
            retry_decision(&RetryPolicy::NONE, 1, None),
            RetryDecision::GiveUp(GiveUpReason::PolicyExhausted)
        );
    }

    #[test]
    fn idle_timeout_gets_at_most_one_retry() {
        let policy = RetryPolicy::default();
        assert!(matches!(
            retry_decision(&policy, 1, Some(RetryHint::timeout())),
            RetryDecision::Retry { .. }
        ));
        assert_eq!(
            retry_decision(&policy, 2, Some(RetryHint::timeout())),
            RetryDecision::GiveUp(GiveUpReason::TimeoutBudget)
        );
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
        assert!((0.0..1.0).contains(&jitter_sample()));
    }
}
