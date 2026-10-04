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
