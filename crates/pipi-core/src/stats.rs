//! 会话统计 —— 移植自 NousResearch/hermes-agent 的 Usage 设计
//! （tui_gateway/server.py 的 `_get_status_bar_snapshot` 与 cli 状态栏）。
//!
//! 语义与上游一致：
//! - **滚动平均**：tok/s 与延迟取最近 N 次 API 调用的聚合
//!   （sum(output)/sum(latency)，N=10），不是逐次平均
//! - **缓存命中率** = cache_read / prompt 总量（prompt = input + cache_read +
//!   cache_write，其中 `input` 只计未命中部分 —— 口径由 provider 适配层归一化，
//!   见 `types::Usage`）
//! - **数据不足时省略而不是编造 0**（上游的原话：omitted, not fabricated）
//! - **上下文占用**用最近一次请求的实际 token，不是累计值

use std::collections::VecDeque;

use serde::Serialize;

use crate::types::{Message, Usage};

/// 滚动窗口大小（hermes：Rolling over the last 10 calls）。
pub const ROLLING_WINDOW: usize = 10;

/// 单次 LLM 调用的观测记录。
#[derive(Debug, Clone, Copy)]
pub struct CallObservation {
    /// 本次生成的输出 token 数。
    pub output_tokens: u64,
    /// 本次生成耗时（秒）。
    pub latency_s: f64,
    /// 最近一次请求的实际输入 token（当前窗口占用）。
    pub prompt_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// 会话级聚合统计，序列化后直接驱动前端状态栏。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    /// 累计输入（不含缓存命中部分）。
    pub input: u64,
    /// 累计输出。
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// LLM 调用次数。
    pub calls: u32,
    /// 滚动平均输出速度（tok/s），最近 N 次调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_tps: Option<f64>,
    /// 滚动平均 API 延迟（秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_latency_s: Option<f64>,
    /// 会话缓存命中率（%），无缓存数据时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_pct: Option<f64>,
    /// 当前上下文占用（最近一次请求的 prompt token）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_used: Option<u64>,
    /// 模型上下文窗口上限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_max: Option<u64>,
    /// 上下文占用百分比。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionStatsTracker {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    calls: u32,
    latency_history: VecDeque<f64>,
    output_history: VecDeque<u64>,
    last: Option<CallObservation>,
    context_max: Option<u64>,
}

impl SessionStatsTracker {
    pub fn new(context_max: Option<u64>) -> Self {
        SessionStatsTracker {
            context_max,
            ..Default::default()
        }
    }

    pub fn set_context_max(&mut self, context_max: Option<u64>) {
        self.context_max = context_max;
    }

    /// 记录一次助手响应（从消息里取 usage 与耗时）。
    pub fn record(&mut self, message: &Message) {
        let Message::Assistant {
            usage,
            duration_ms,
            ..
        } = message
        else {
            return;
        };
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.calls += 1;

        let latency_s = duration_ms.map(|ms| ms as f64 / 1000.0);
        if let (Some(latency), true) = (latency_s, usage.output > 0) {
            self.latency_history.push_back(latency);
            self.output_history.push_back(usage.output);
            while self.latency_history.len() > ROLLING_WINDOW {
                self.latency_history.pop_front();
            }
            while self.output_history.len() > ROLLING_WINDOW {
                self.output_history.pop_front();
            }
        }

        // 最近一次请求的实际窗口占用：input + cache_read + cache_write
        let prompt = usage.prompt_tokens();
        self.last = Some(CallObservation {
            output_tokens: usage.output,
            latency_s: latency_s.unwrap_or(0.0),
            prompt_tokens: prompt,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
        });
    }

    /// 只进累计账本的用量：摘要压缩这类**一次性调用**用它。
    ///
    /// 不计入滚动平均，也不改写「最近一次调用」口径 —— 摘要请求的 prompt 是
    /// 被压缩的原文，不是当前上下文占用，拿它当窗口占用会误导面板。
    pub fn record_ledger(&mut self, usage: &Usage) {
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.calls += 1;
    }

    pub fn snapshot(&self) -> SessionStats {
        let mut stats = SessionStats {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            calls: self.calls,
            ..Default::default()
        };

        // 滚动 tps / latency：sum(output[-N:]) / sum(latency[-N:])（hermes 算法）
        let n = self.latency_history.len().min(self.output_history.len());
        if n > 0 {
            let total_lat: f64 = self.latency_history.iter().sum();
            let total_out: u64 = self.output_history.iter().sum();
            if total_lat > 0.0 {
                let tps = total_out as f64 / total_lat;
                // 上游对 NaN/负数/荒谬值的防护
                if tps.is_finite() && tps > 0.0 && tps < 1e6 {
                    stats.avg_tps = Some((tps * 10.0).round() / 10.0);
                }
                stats.avg_latency_s = Some((total_lat / n as f64 * 100.0).round() / 100.0);
            }
        }

        // 缓存命中率：cache_read / prompt 总量（数据不足则省略）
        if let Some(last) = &self.last {
            let prompt_total = last.prompt_tokens;
            if prompt_total > 0 && last.cache_read > 0 {
                stats.cache_hit_pct =
                    Some((last.cache_read as f64 / prompt_total as f64 * 100.0).min(100.0));
            }
            stats.context_used = Some(prompt_total);
        }
        stats.context_max = self.context_max;
        if let (Some(used), Some(max)) = (stats.context_used, self.context_max) {
            if max > 0 {
                stats.context_percent = Some(((used as f64 / max as f64) * 100.0).min(100.0) as u64);
            }
        }
        stats
    }
}

/// 单条消息的统计（聊天 UI 的消息脚注）。
/// 返回 (tok/s, 缓存命中率%)。
pub fn message_stats(message: &Message) -> (Option<f64>, Option<f64>) {
    let Message::Assistant {
        usage,
        duration_ms,
        ..
    } = message
    else {
        return (None, None);
    };
    let tps = duration_ms
        .filter(|ms| *ms > 0)
        .map(|ms| usage.output as f64 / (ms as f64 / 1000.0));
    let prompt_total = usage.prompt_tokens();
    let cache_hit = if prompt_total > 0 && usage.cache_read > 0 {
        Some((usage.cache_read as f64 / prompt_total as f64 * 100.0).min(100.0))
    } else {
        None
    };
    (tps, cache_hit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, StopReason, Usage};

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
}
