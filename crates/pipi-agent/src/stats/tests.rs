use super::*;
use pipi_protocol::{ContentBlock, StopReason, Usage};

fn assistant(usage: Usage, duration_ms: u64) -> Message {
    Message::Assistant {
        content: vec![ContentBlock::Text { text: "x".into() }],
        api: String::new(),
        provider: String::new(),
        model: "m".into(),
        usage,
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
        duration_ms: Some(duration_ms),
    }
}

#[test]
fn rolling_tps_and_cache_hit() {
    let mut tracker = SessionStatsTracker::new(Some(100_000));
    // 两次调用：各 200 tok / 2s = 100 tok/s；缓存命中 50/100
    tracker.record(&assistant(
        Usage {
            input: 50,
            output: 200,
            cache_read: 50,
            cache_write: 0,
            total_tokens: 300,
        },
        2_000,
    ));
    tracker.record(&assistant(
        Usage {
            input: 50,
            output: 300,
            cache_read: 60,
            cache_write: 0,
            total_tokens: 410,
        },
        3_000,
    ));

    let stats = tracker.snapshot();
    // 滚动窗口：500 tok / 5s = 100 tok/s
    assert_eq!(stats.avg_tps, Some(100.0));
    assert_eq!(stats.avg_latency_s, Some(2.5));
    // 命中率取最近一次：60 / (50+60) ≈ 54.5%
    let hit = stats.cache_hit_pct.unwrap();
    assert!((54.0..=55.5).contains(&hit));
    assert_eq!(stats.context_used, Some(110));
    assert_eq!(stats.context_max, Some(100_000));
    assert_eq!(stats.context_percent, Some(0)); // 110/100000 → 0%
    assert_eq!(stats.calls, 2);
    assert_eq!(stats.output, 500);
}

#[test]
fn window_keeps_last_ten() {
    let mut tracker = SessionStatsTracker::new(None);
    for i in 0..15 {
        tracker.record(&assistant(
            Usage {
                input: 0,
                output: 100,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 100,
            },
            1_000 + i * 1_000, // 递增延迟：早的 1s，晚的 15s
        ));
    }
    let stats = tracker.snapshot();
    // 窗口只含最近 10 次：延迟 6s..15s，均值 10.5s；输出 100*10/总延迟
    assert_eq!(stats.avg_latency_s, Some(10.5));
    let tps = stats.avg_tps.unwrap();
    // 1000 tok / 105s（最近 10 次延迟 6..15s）≈ 9.5
    assert!((9.0..=10.0).contains(&tps), "tps={tps}");
}

#[test]
fn omit_stats_without_data() {
    // 无缓存数据：命中率省略而不是 0
    let mut tracker = SessionStatsTracker::new(None);
    tracker.record(&assistant(
        Usage {
            input: 100,
            output: 50,
            cache_read: 0,
            cache_write: 0,
            total_tokens: 150,
        },
        1_000,
    ));
    let stats = tracker.snapshot();
    assert_eq!(stats.cache_hit_pct, None);
    assert_eq!(stats.avg_tps, Some(50.0));
    // 非 assistant 消息不计入
    tracker.record(&Message::user_text("hi"));
    assert_eq!(tracker.snapshot().calls, 1);
}

#[test]
fn cache_hit_uses_uncached_input_only() {
    // 回归：OpenAI 兼容端点上报的 prompt_tokens 已含缓存部分，适配层扣掉之后
    // `input` 只剩未命中量。命中率必须按 input + cache_read 算 —— 若哪天又把
    // cache_read 加回 input（重复计数），这里会掉回 50%。
    let mut tracker = SessionStatsTracker::new(Some(1_000_000));
    tracker.record(&assistant(
        Usage {
            input: 134,
            output: 165,
            cache_read: 610_432,
            cache_write: 0,
            total_tokens: 610_731,
        },
        1_000,
    ));
    let stats = tracker.snapshot();
    let hit = stats.cache_hit_pct.unwrap();
    assert!((hit - 99.98).abs() < 0.01, "hit={hit}");
    assert_eq!(stats.context_used, Some(610_566));
    let (_, per_message) = message_stats(&assistant(
        Usage {
            input: 134,
            output: 165,
            cache_read: 610_432,
            cache_write: 0,
            total_tokens: 610_731,
        },
        1_000,
    ));
    assert!((per_message.unwrap() - 99.98).abs() < 0.01);
}

#[test]
fn ledger_records_summary_cost_without_touching_last_call() {
    let mut tracker = SessionStatsTracker::new(Some(1_000_000));
    tracker.record(&assistant(
        Usage {
            input: 100,
            output: 50,
            cache_read: 900,
            cache_write: 0,
            total_tokens: 1050,
        },
        1_000,
    ));
    let before = tracker.snapshot();
    // 摘要调用：进累计账本（input/output/cache/calls），不动窗口占用与命中率
    tracker.record_ledger(&Usage {
        input: 40_000,
        output: 800,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 40_800,
    });
    let after = tracker.snapshot();
    assert_eq!(after.input, before.input + 40_000);
    assert_eq!(after.output, before.output + 800);
    assert_eq!(after.calls, before.calls + 1);
    assert_eq!(after.context_used, before.context_used);
    assert_eq!(after.cache_hit_pct, before.cache_hit_pct);
    assert_eq!(after.avg_tps, before.avg_tps);
}

#[test]
fn per_message_stats() {
    let (tps, hit) = message_stats(&assistant(
        Usage {
            input: 30,
            output: 120,
            cache_read: 90,
            cache_write: 0,
            total_tokens: 240,
        },
        2_000,
    ));
    assert_eq!(tps, Some(60.0));
    // 命中率 = 90 / (30 + 90) = 75%
    assert_eq!(hit, Some(75.0));
    let (tps, hit) = message_stats(&Message::user_text("x"));
    assert_eq!((tps, hit), (None, None));
}
