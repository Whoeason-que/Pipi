use super::*;
use crate::context::estimate_tokens;
use crate::test_support::{assistant_text, tool_result};
use pipi_protocol::{ContentBlock, StopReason};

fn tool_call(name: &str, path: &str) -> Message {
    Message::Assistant {
        content: vec![ContentBlock::ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: serde_json::json!({ "path": path }),
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: Default::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
        duration_ms: None,
    }
}

#[test]
fn file_operations_extracts_read_and_modified_paths() {
    let messages = vec![
        Message::user_text("go"),
        tool_call("read", "b.rs"),
        tool_call("read", "a.rs"),
        tool_call("write", "a.rs"),
        tool_call("edit", "c.rs"),
        tool_call("bash", "irrelevant"),
        tool_result("t1", "bash", "done"),
    ];
    let (read, modified) = file_operations(&messages);
    assert_eq!(read, vec!["a.rs".to_string(), "b.rs".to_string()]);
    assert_eq!(modified, vec!["a.rs".to_string(), "c.rs".to_string()]);
}

#[test]
fn budget_derives_threshold_from_window_percent() {
    // 默认 75%：20 万窗口 → 15 万触发
    let budget = Budget::from_window(200_000, DEFAULT_THRESHOLD_PERCENT);
    assert_eq!(budget.trigger_tokens(), 150_000);
    assert_eq!(budget.summary_max, SUMMARY_MAX_TOKENS);
    assert_eq!(budget.keep_budget(), KEEP_RECENT_TOKENS);
    assert_eq!(budget.keep_budget_for(440_000), KEEP_RECENT_TOKENS);
    let target = budget.with_target_percent(Some(20));
    assert_eq!(target.trigger_tokens(), budget.trigger_tokens());
    assert_eq!(
        target.keep_budget_for(440_000),
        88_000 - SUMMARY_MAX_TOKENS as u64
    );
    assert_eq!(
        target.with_target_percent(Some(0)).keep_budget_for(440_000),
        KEEP_RECENT_TOKENS
    );
    // 百分比可调：50% 更早压
    assert_eq!(Budget::from_window(200_000, 50).trigger_tokens(), 100_000);
    // 窗口小：保留预算被触发线压住，不会超过可用空间
    let small = Budget::from_window(30_000, 75);
    assert_eq!(small.summary_max, 1_529);
    assert_eq!(
        small.keep_budget(),
        22_500 - DEFAULT_RESERVE_TOKENS - small.summary_max as u64 * 2
    );
    // 越界百分比退回默认（手改 agent.json 不该让压缩失效）
    assert_eq!(Budget::from_window(200_000, 0).threshold_percent, 75);
    assert_eq!(Budget::from_window(200_000, 200).threshold_percent, 75);
    assert_eq!(normalize_threshold_percent(1), 1);
    assert_eq!(normalize_threshold_percent(100), 100);
    assert_eq!(estimate_tokens(&Message::user_text("abcdefgh")), 2);
}

#[test]
fn needs_compaction_respects_zero_window() {
    assert!(!needs_compaction(
        &[Message::user_text("hi")],
        Budget::from_window(0, 75)
    ));
    assert!(needs_compaction(
        &[Message::user_text("x".repeat(100_000))],
        Budget::from_window(10_000, 75)
    ));
}

#[test]
fn validate_rejects_illegal_plans() {
    let messages = vec![
        Message::user_text("hi"),
        assistant_text("ok"),
        tool_result("t1", "bash", "body"),
    ];
    // 下标越界
    assert!(
        validate(
            &messages,
            &Plan {
                rewrites: vec![(9, tool_result("t1", "bash", "x"))],
                ..Default::default()
            }
        )
        .is_err()
    );
    // 就地改写不得改角色
    assert!(
        validate(
            &messages,
            &Plan {
                rewrites: vec![(2, assistant_text("x"))],
                ..Default::default()
            }
        )
        .is_err()
    );
    // 就地改写与保留混用（下标会错位）
    assert!(
        validate(
            &messages,
            &Plan {
                rewrites: vec![(2, tool_result("t1", "bash", "x"))],
                keep_from: Some(1),
                ..Default::default()
            }
        )
        .is_err()
    );
    // 保留起点落在工具结果上
    assert!(
        validate(
            &messages,
            &Plan {
                keep_from: Some(2),
                ..Default::default()
            }
        )
        .is_err()
    );
    // 合法：只保留尾部
    assert!(
        validate(
            &messages,
            &Plan {
                keep_from: Some(1),
                ..Default::default()
            }
        )
        .is_ok()
    );
}

#[test]
fn plan_apply_keeps_tail_and_injects_summary() {
    let messages = vec![
        Message::user_text("old"),
        assistant_text("old answer"),
        Message::user_text("new"),
    ];
    let plan = Plan {
        keep_from: Some(2),
        injections: vec![crate::session::summary_message("旧摘要")],
        ..Default::default()
    };
    let out = plan.apply(&messages);
    assert_eq!(out.len(), 2);
    assert_eq!(crate::session::summary_text(&out[0]), Some("旧摘要"));
    assert_eq!(out[1].role(), "user");
    validate_result(&messages, &out).unwrap();
}

#[test]
fn validate_result_only_rejects_new_damage() {
    // 输入本来就有的破损不算在压缩头上（中段悬挂由发送前兜底）
    let orphan = |id: &str| Message::ToolResult {
        tool_call_id: id.into(),
        tool_name: "bash".into(),
        content: vec![pipi_protocol::ToolResultContent::Text { text: "x".into() }],
        is_error: false,
        details: None,
        timestamp: 0,
    };
    let already_broken = vec![Message::user_text("hi"), orphan("o1")];
    assert!(validate_result(&already_broken, &already_broken.clone()).is_ok());
    // 新增孤儿结果 → 拒绝
    let mut worse = already_broken.clone();
    worse.push(orphan("o2"));
    assert!(validate_result(&already_broken, &worse).is_err());
    // 新增无主调用 → 拒绝
    let mut missing = vec![Message::user_text("hi")];
    missing.push(assistant_call("t9"));
    assert!(validate_result(&[Message::user_text("hi")], &missing).is_err());
    let healthy = vec![
        Message::user_text("hi"),
        Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        },
        tool_result("t1", "bash", "ok"),
    ];
    assert!(validate_result(&healthy, &healthy.clone()).is_ok());
    // 保留段切在工具结果上会孤立它 → 拒绝
    let cut = vec![healthy[2].clone()];
    assert!(validate_result(&healthy, &cut).is_err());
}

fn assistant_call(id: &str) -> Message {
    Message::Assistant {
        content: vec![ContentBlock::ToolCall {
            id: id.into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: Default::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
        duration_ms: None,
    }
}

#[test]
fn projection_pipeline_is_noop_without_pressure() {
    let messages = vec![
        Message::user_text("hi"),
        assistant_text("ok"),
        tool_result("t1", "bash", &"x".repeat(20_000)),
    ];
    // 窗口足够大：既不清理也不裁剪（细节保留到真需要时）
    assert_eq!(
        project(
            messages.clone(),
            Budget::from_window(1_000_000, DEFAULT_THRESHOLD_PERCENT)
        ),
        messages
    );
}

#[test]
fn projection_pipeline_clears_tool_outputs_before_hard_pruning() {
    // 多轮历史：每轮一个很大的工具输出，整体超预算
    let mut messages = Vec::new();
    for turn in 0..8 {
        messages.push(Message::user_text(format!("问题 {turn}")));
        messages.push(assistant_text(&"思考".repeat(50)));
        messages.push(tool_result("t1", "bash", &"输出".repeat(8_000)));
    }
    // 阈值从实测估算值反推（CJK 按字节算，别口算）：设在「清掉最旧 2 个
    // 工具输出就落回阈值内」的位置，于是只有清理档生效、硬裁不该触发
    let total = estimate_context_tokens(&messages);
    let per_result = estimate_tokens(&messages[2]);
    let cleared = 8 - TOOL_OUTPUT_KEEP;
    let trigger = total - per_result * cleared as u64 + 100;
    // 触发线直接用窗口表达（百分比 100 = 触发线即窗口），与百分比口径解耦
    let budget = Budget {
        context_window: trigger,
        threshold_percent: 100,
        ..Budget::from_window(trigger, 100)
    };
    assert!(total > budget.trigger_tokens(), "总 {total} 应超阈值");
    let after = project(messages, budget);
    // 结构不变：条数一致、工具结果仍在原位（配对不受影响）
    assert_eq!(after.len(), 24);
    validate_result(&after, &after.clone()).unwrap();
    let tool_outputs: Vec<&Message> = after.iter().filter(|m| m.role() == "toolResult").collect();
    assert_eq!(tool_outputs.len(), 8);
    // 前 8-KEEP 个被清理，最近 KEEP 个保留原文
    for message in &tool_outputs[..8 - TOOL_OUTPUT_KEEP] {
        let Message::ToolResult { content, .. } = message else {
            unreachable!()
        };
        let pipi_protocol::ToolResultContent::Text { text } = &content[0] else {
            unreachable!()
        };
        assert!(
            text.starts_with(CLEARED_TOOL_RESULT_PREFIX),
            "旧输出应被清理: {text}"
        );
    }
    let Message::ToolResult { content, .. } = tool_outputs[7] else {
        unreachable!()
    };
    let pipi_protocol::ToolResultContent::Text { text } = &content[0] else {
        unreachable!()
    };
    assert!(
        !text.starts_with(CLEARED_TOOL_RESULT_PREFIX),
        "最近的输出应保留原文"
    );
}

#[test]
fn projection_is_idempotent() {
    let mut messages = Vec::new();
    for turn in 0..8 {
        messages.push(Message::user_text(format!("问题 {turn}")));
        messages.push(tool_result("t1", "bash", &"输出".repeat(8_000)));
    }
    let total = estimate_context_tokens(&messages);
    let per_result = estimate_tokens(&messages[1]);
    let trigger = total - per_result * 4 + 100;
    let budget = Budget {
        context_window: trigger,
        threshold_percent: 100,
        ..Budget::from_window(trigger, 100)
    };
    let once = project(messages, budget);
    let twice = project(once.clone(), budget);
    assert_eq!(once, twice, "投影必须可重复应用（回放时重算不会变形）");
}

mod llm {
    use crate::compaction::llm::{LlmSummarize, build_summary_prompt, find_keep_start, summarize};
    use crate::compaction::{Preparation, Replacement, StrategyEnv};
    use crate::test_support::{assistant_text, tool_result};
    use pipi_protocol::{
        AbortSignal, Api, ContentBlock, Context, Message, Model, StopReason, StreamEvent,
        StreamOptions, Usage,
    };
    use pipi_provider::{EventStream, Provider};
    use std::sync::Mutex;

    struct SummaryProvider {
        reason: StopReason,
        content: Vec<ContentBlock>,
        usage: Usage,
        requested_max_tokens: Mutex<Vec<u32>>,
    }

    #[async_trait::async_trait]
    impl Provider for SummaryProvider {
        async fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            options: &StreamOptions,
            _abort: AbortSignal,
        ) -> EventStream {
            self.requested_max_tokens
                .lock()
                .unwrap()
                .push(options.max_tokens.unwrap());
            let message = Message::Assistant {
                content: self.content.clone(),
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: self.usage,
                stop_reason: self.reason,
                error_message: None,
                timestamp: 0,
                duration_ms: None,
            };
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            tx.send(StreamEvent::Done {
                reason: self.reason,
                usage: self.usage,
                message: Box::new(message),
            })
            .await
            .unwrap();
            rx
        }
    }

    async fn summary_reply(
        model_max: u32,
        summary_max: u32,
        reason: StopReason,
        text: &str,
        output_tokens: u64,
    ) -> (Result<(String, Option<Usage>), String>, u32) {
        let content = if text.is_empty() {
            vec![ContentBlock::Thinking {
                thinking: "reasoning only".into(),
                thinking_signature: None,
            }]
        } else {
            vec![ContentBlock::Text { text: text.into() }]
        };
        let provider = SummaryProvider {
            reason,
            content,
            usage: Usage {
                output: output_tokens,
                ..Default::default()
            },
            requested_max_tokens: Mutex::new(Vec::new()),
        };
        let model = Model {
            id: "test".into(),
            name: String::new(),
            api: Api::OpenAICompletions,
            base_url: String::new(),
            max_tokens: model_max,
            context_window: 1_000_000,
        };
        let options = StreamOptions::default();
        let env = StrategyEnv {
            provider: &provider,
            model: &model,
            options: &options,
            abort: AbortSignal::new(),
            retry: crate::retry::RetryPolicy::NONE,
        };
        let result = summarize(&[Message::user_text("history")], None, summary_max, &env).await;
        let requested = provider.requested_max_tokens.lock().unwrap();
        (result, requested[0])
    }

    #[tokio::test]
    async fn summary_request_uses_budget_bounded_by_model_limit() {
        let budget = super::super::Budget::from_window(1_000_000, 75);
        let (result, requested) = summary_reply(
            384_000,
            budget.summary_max,
            StopReason::Stop,
            "complete summary",
            500,
        )
        .await;
        assert_eq!(requested, super::super::SUMMARY_MAX_TOKENS);
        assert_eq!(result.unwrap().0, "complete summary");

        let (result, requested) =
            summary_reply(2_048, budget.summary_max, StopReason::Stop, "summary", 100).await;
        assert_eq!(requested, 2_048);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn length_stop_does_not_persist_partial_summary() {
        let (result, requested) =
            summary_reply(384_000, 4_096, StopReason::Length, "partial summary", 4_096).await;
        assert_eq!(requested, 4_096);
        assert!(result.unwrap_err().contains("未保存可能截断的摘要"));
    }

    #[tokio::test]
    async fn reasoning_only_reply_reports_output_usage() {
        let (result, requested) = summary_reply(384_000, 4_096, StopReason::Stop, "", 1_024).await;
        assert_eq!(requested, 4_096);
        let error = result.unwrap_err();
        assert!(error.contains("模型未返回摘要内容"));
        assert!(error.contains("输出 1024 个 token"));
    }

    #[tokio::test]
    async fn target_percent_changes_the_kept_user_turns() {
        let provider = SummaryProvider {
            reason: StopReason::Stop,
            content: vec![ContentBlock::Text {
                text: "summary".into(),
            }],
            usage: Usage::default(),
            requested_max_tokens: Mutex::new(Vec::new()),
        };
        let model = Model {
            id: "test".into(),
            name: String::new(),
            api: Api::OpenAICompletions,
            base_url: String::new(),
            max_tokens: 8_192,
            context_window: 100_000,
        };
        let options = StreamOptions::default();
        let env = StrategyEnv {
            provider: &provider,
            model: &model,
            options: &options,
            abort: AbortSignal::new(),
            retry: crate::retry::RetryPolicy::NONE,
        };
        let messages = vec![
            Message::user_text("a".repeat(40_000)),
            Message::user_text("b".repeat(6_000)),
            Message::user_text("c".repeat(6_000)),
        ];
        let budget = super::super::Budget {
            summary_max: 100,
            keep_recent: 2_000,
            ..super::super::Budget::from_window(100_000, 75)
        };
        let legacy = Preparation::new(&messages, budget);
        let strategy = LlmSummarize;
        assert_eq!(
            strategy.run(&legacy, &env).await.unwrap().keep_from,
            Some(2)
        );

        let target = Preparation::new(&messages, budget.with_target_percent(Some(50)));
        assert_eq!(
            strategy.run(&target, &env).await.unwrap().keep_from,
            Some(1)
        );
    }

    #[test]
    fn renders_all_message_kinds() {
        let transcript = build_summary_prompt(
            &[
                Message::user_text("fix the bug"),
                assistant_text("let me check"),
                tool_result("t1", "bash", "ok"),
            ],
            None,
        );
        assert!(transcript.contains("[user]"));
        assert!(transcript.contains("fix the bug"));
        assert!(transcript.contains("[assistant]"));
        assert!(
            transcript.contains("[assistant tool call:") || transcript.contains("let me check")
        );
        assert!(transcript.contains("[tool result: bash (ok)]"));
        assert!(transcript.contains("<conversation>"));
        assert!(transcript.contains("## Goal"));
        assert!(transcript.contains("## Critical Context"));
        // 无前摘要时不含 previous-summary 节
        assert!(!transcript.contains("<previous-summary>"));
    }

    #[test]
    fn summary_prompt_includes_previous_summary_and_update_instruction() {
        let transcript =
            build_summary_prompt(&[Message::user_text("next turn")], Some("## Goal\n旧摘要"));
        assert!(transcript.contains("<previous-summary>\n## Goal\n旧摘要\n</previous-summary>"));
        assert!(transcript.contains("PRESERVE all existing information"));
        assert!(!transcript.contains("Use EXACTLY this skeleton"));
    }

    #[test]
    fn keep_start_cuts_at_user_boundary_only() {
        // user(长) → assistant → toolResult → user(短) → assistant(短)
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text(&"y".repeat(100)),
            tool_result("t1", "bash", &"z".repeat(100)),
            Message::user_text("recent question"),
            assistant_text("recent answer"),
        ];
        // 预算只够最近两条：切点必须落在 index 3（user 边界）
        let start = find_keep_start(&messages, 200).unwrap();
        assert_eq!(start, 3);
        assert_eq!(messages[start].role(), "user");
    }
    #[test]
    fn keep_start_is_none_when_all_fit_and_never_orphans_tool_results() {
        // 全部装得下：不需要保留段
        let messages = vec![Message::user_text("hello"), assistant_text("hi")];
        assert!(find_keep_start(&messages, 10_000).is_none());

        // 尾部紧邻 toolResult、更早处无 user 边界时：保留空段（全部摘要化），
        // 绝不孤立 toolResult
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text("a"),
            tool_result("t1", "bash", "r"),
        ];
        let start = find_keep_start(&messages, 100).unwrap();
        assert_eq!(start, messages.len());
    }
}

mod tool_output {
    use crate::compaction::tool_output::{CLEARED_TOOL_RESULT_PREFIX, ToolOutputPrune};
    use crate::compaction::{Budget, DEFAULT_THRESHOLD_PERCENT, Preparation, Projection};
    use crate::test_support::tool_result;
    use pipi_protocol::{ContentBlock, Message, ToolResultContent};

    fn big_history(turns: usize) -> Vec<Message> {
        let mut messages = Vec::new();
        for turn in 0..turns {
            messages.push(Message::user_text(format!("问题 {turn}")));
            messages.push(Message::Assistant {
                content: vec![ContentBlock::Text {
                    text: "答".repeat(20),
                }],
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Default::default(),
                stop_reason: pipi_protocol::StopReason::Stop,
                error_message: None,
                timestamp: 0,
                duration_ms: None,
            });
            messages.push(tool_result("t", "bash", &"输出".repeat(8_000)));
        }
        messages
    }

    fn budget() -> Budget {
        Budget {
            context_window: 40_000,
            ..Budget::from_window(40_000, DEFAULT_THRESHOLD_PERCENT)
        }
    }

    #[test]
    fn clears_only_old_and_large_results() {
        let messages = big_history(8);
        let prep = Preparation::new(&messages, budget());
        let plan = ToolOutputPrune.plan(&prep);
        // 8 个工具结果，最近 6 个保留 → 清 2 个
        assert_eq!(plan.rewrites.len(), 2);
        // 清的都是早期下标，且保持工具结果身份（配对不受影响）
        for (index, replacement) in &plan.rewrites {
            assert!(messages[*index].role() == "toolResult");
            assert_eq!(replacement.role(), "toolResult");
            let Message::ToolResult { content, .. } = replacement else {
                unreachable!()
            };
            let ToolResultContent::Text { text } = &content[0] else {
                unreachable!()
            };
            assert!(text.starts_with(CLEARED_TOOL_RESULT_PREFIX));
            assert!(text.contains("bash"));
        }
    }

    #[test]
    fn skips_short_results_and_already_cleared_ones() {
        let mut messages = big_history(8);
        // 把一个早期结果换成很短的内容：不值得清理
        messages[2] = tool_result("t", "bash", "ok");
        let prep = Preparation::new(&messages, budget());
        let plan = ToolOutputPrune.plan(&prep);
        assert!(plan.rewrites.iter().all(|(index, _)| *index != 2));

        // 二次应用：已清理的不再出现在计划里（幂等）
        let applied = plan.apply(&messages);
        let again = ToolOutputPrune.plan(&Preparation::new(&applied, budget()));
        assert!(again.rewrites.is_empty(), "已清理的结果不应重复清理");
    }

    #[test]
    fn keeps_error_flag_so_failures_stay_visible() {
        let mut messages = big_history(8);
        let Message::ToolResult {
            content,
            tool_name,
            tool_call_id,
            details,
            timestamp,
            ..
        } = messages[2].clone()
        else {
            unreachable!()
        };
        messages[2] = Message::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error: true,
            details,
            timestamp,
        };
        let prep = Preparation::new(&messages, budget());
        let plan = ToolOutputPrune.plan(&prep);
        let (_, replacement) = plan
            .rewrites
            .iter()
            .find(|(index, _)| *index == 2)
            .expect("失败的旧输出也应被清理");
        let Message::ToolResult { is_error, .. } = replacement else {
            unreachable!()
        };
        assert!(*is_error, "失败标记必须保留：模型仍要看到这次调用失败了");
    }

    #[test]
    fn does_not_apply_without_pressure() {
        let messages = big_history(8);
        // 窗口足够大：不清理（细节留到真需要时）
        let roomy = Budget::from_window(1_000_000, DEFAULT_THRESHOLD_PERCENT);
        assert!(!ToolOutputPrune.applies(&Preparation::new(&messages, roomy)));
        assert!(ToolOutputPrune.applies(&Preparation::new(&messages, budget())));
    }
}
