//! 上下文预算（纯函数）：token 估算、裁剪、工具配对修复。
//!
//! - `estimate_tokens` / `prune_cut_index` 移植自
//!   `packages/agent/src/harness/compaction/compaction.ts` 的启发式部分
//!   （chars/4、图片按 4800 chars 计）。估算只是兜底：有效的助手 usage 存在时，
//!   `context_tokens` 以真实占用为锚、只估算之后的增量（同 pi 的 `estimateContextTokens`）。上游的完整 compaction 用 LLM 做
//!   摘要替换，属「有意推迟」；这里的切点规则由压缩管线（`compaction`）使用。
//! - 环境上下文的注入（`environment_context`，移植自 codex）随提示词层落在
//!   `pipi-venv/src/prompt/environment.rs` —— 这个 crate 只做策略，不装配提示词。

use pipi_protocol::{ContentBlock, Message, StopReason, ToolResultContent, Usage};

/// 为 compaction 预留的 token（经验值，对齐 pi 的预留思路）。
pub const DEFAULT_RESERVE_TOKENS: u64 = 16_384;

/// 图片内容块的估算字符数（pi 的 ESTIMATED_IMAGE_CHARS）。
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// 估算单条消息的 token（pi 的 chars/4 保守启发式）。
///
/// 注意用**字符数**而不是字节数：Rust 的 `String::len()` 是 UTF-8 字节长度，
/// 中文一个字算 3 个，会让触发线和保留预算整体提前（CJK 会话压缩过早）。
/// 未知模型的真实口径无从得知，这里保持 pi 的字符启发式，不做模型级 tokenizer。
pub fn estimate_tokens(message: &Message) -> u64 {
    fn chars(text: &str) -> usize {
        text.chars().count()
    }

    let count: usize = match message {
        Message::User { content, .. } => chars(content),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => chars(text),
                ContentBlock::Thinking { thinking, .. } => chars(thinking),
                ContentBlock::ToolCall {
                    name, arguments, ..
                } => {
                    chars(name)
                        + serde_json::to_string(arguments)
                            .map(|json| chars(&json))
                            .unwrap_or_default()
                }
            })
            .sum(),
        Message::ToolResult { content, .. } => content
            .iter()
            .map(|c| match c {
                ToolResultContent::Text { text } => chars(text),
                ToolResultContent::Image { .. } => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
    };
    count.div_ceil(4) as u64
}

/// 估算整段消息历史的 token（纯估算，不看 usage）。
pub fn estimate_context_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_tokens).sum()
}

/// 一次请求的上下文占用：供应商 `total_tokens`（缺失时由分项相加）。
fn usage_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.total()
    }
}

/// 找到能代表当前前缀的最近一条助手 usage，返回（下标，占用 token）。
///
/// 规则对齐 pi 的 `getLastAssistantUsageInfo`：
/// - `aborted` / `error` 的轮次不计（用量不完整）；占用为 0 的也不计；
/// - 若某条较新的前缀消息（如压缩摘要）插在它之前，它的用量描述的是旧前缀，
///   不再适用 —— 判据是「它的时间戳 ≥ 之前所有消息的最大时间戳」。
pub fn last_usage_anchor(messages: &[Message]) -> Option<(usize, u64)> {
    let mut latest_prefix_ts = 0u64;
    let mut anchor = None;
    for (index, message) in messages.iter().enumerate() {
        if let Message::Assistant {
            usage,
            stop_reason,
            timestamp,
            ..
        } = message
            && *timestamp >= latest_prefix_ts
            && !matches!(stop_reason, StopReason::Aborted | StopReason::Error)
        {
            let tokens = usage_context_tokens(usage);
            if tokens > 0 {
                anchor = Some((index, tokens));
            }
        }
        latest_prefix_ts = latest_prefix_ts.max(message.timestamp());
    }
    anchor
}

/// 当前上下文占用（触发判断与压缩前后统计用）：
/// 有锚点时 = 锚点的真实占用 + 锚点之后消息的估算；无锚点时整体估算。
///
/// 投影式策略（清理 / 硬裁）**不用它**：它们只依赖原始历史、必须可重算；
/// 锚点来自旧请求，清理后不会下降，拿来判断会过度裁剪。
pub fn context_tokens(messages: &[Message]) -> u64 {
    match last_usage_anchor(messages) {
        Some((index, tokens)) => tokens + estimate_context_tokens(&messages[index + 1..]),
        None => estimate_context_tokens(messages),
    }
}

/// 保底裁剪的切点决策：返回「保留段起点」的下标，`None` 表示无需裁剪。
///
/// `budget_tokens` 是整段历史允许占用的上限（调用方给压缩触发线，见
/// `compaction::Budget::trigger_tokens`）—— 预算口径只在那一处计算。
///
/// 切点只落在 user 消息的开头 —— toolResult 永远不会与其 assistant 分离
/// （provider 会拒绝孤儿 toolResult）。找不到可行切点（尾部本身就超预算）
/// 时返回 `None`，交由上层处理（provider 会以 length 停止，loop 已有对应
/// 处理路径）。
///
/// 投影式压缩策略（如旧工具输出清理）需要「切在哪」而不是「切完是什么」，
/// 所以决策与执行分开：本函数只回答切点，应用由调用方（`compaction`）完成。
pub fn prune_cut_index(messages: &[Message], budget_tokens: u64) -> Option<usize> {
    let budget = budget_tokens;
    let total = estimate_context_tokens(messages);
    if total <= budget || messages.is_empty() {
        return None;
    }

    // 前缀累计 token
    let mut cum = 0u64;
    for (i, message) in messages.iter().enumerate() {
        if message.role() == "user" && i > 0 {
            let tail = total - cum;
            if tail <= budget {
                return Some(i);
            }
        }
        cum += estimate_tokens(message);
    }
    None
}

/// 结果缺失时写给模型看的合成 tool 结果文案（发送前修复与载入自愈共用）。
pub const MISSING_TOOL_RESULT_TEXT: &str =
    "Tool result missing: this turn was interrupted (app restart / force stop), and this call produced no output.";

/// 合成一条「结果缺失」的失败 tool 结果。
pub fn missing_tool_result(tool_call_id: &str, tool_name: &str) -> Message {
    Message::ToolResult {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: vec![ToolResultContent::Text {
            text: MISSING_TOOL_RESULT_TEXT.into(),
        }],
        is_error: true,
        details: None,
        timestamp: pipi_protocol::now_millis(),
    }
}

/// 工具配对修复：保证「带 tool_calls 的 assistant 消息」后面紧跟回答每个
/// `tool_call_id` 的 tool 结果 —— 缺失的补一条合成错误结果，不回答任何前置
/// 调用的孤儿结果丢弃。
///
/// 协议硬校验这对配对（OpenAI：「An assistant message with 'tool_calls' must
/// be followed by tool messages responding to each 'tool_call_id'」；Anthropic
/// 同理），历史里任何来源的破损（进程中断、批次中止、手工编辑）都会让整个
/// 会话无法继续。这里是**发送前**的最后一道闸（对齐 opencode
/// `session/message-v2.ts` 对 pending/running 工具调用补 output-error 的做法）：
/// 不写盘、不改变会话记录，只保证发出去的请求合法。
pub fn repair_tool_pairing(messages: Vec<Message>) -> Vec<Message> {
    let needs_repair = messages.iter().any(|message| {
        matches!(message, Message::ToolResult { .. }) || !message.tool_calls().is_empty()
    });
    if !needs_repair {
        return messages;
    }

    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    // 尚未被回答的调用：(id, 工具名)
    let mut pending: Vec<(String, String)> = Vec::new();
    for message in messages {
        match &message {
            Message::ToolResult { tool_call_id, .. } => {
                // 只保留回答前置调用的结果（顺带保证每个调用只被回答一次）；
                // 找不到对应调用的孤儿结果丢弃 —— 协议同样会拒绝。
                if let Some(position) = pending.iter().position(|(id, _)| id == tool_call_id) {
                    pending.remove(position);
                    out.push(message);
                }
            }
            Message::Assistant { .. } => {
                flush_pending(&mut out, &mut pending);
                pending = message
                    .tool_calls()
                    .iter()
                    .filter_map(|call| match call {
                        ContentBlock::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
                        _ => None,
                    })
                    .collect();
                out.push(message);
            }
            Message::User { .. } => {
                // 用户消息也要先封口：tool 结果必须紧跟它回答的 assistant 消息
                flush_pending(&mut out, &mut pending);
                out.push(message);
            }
        }
    }
    flush_pending(&mut out, &mut pending);
    out
}

fn flush_pending(out: &mut Vec<Message>, pending: &mut Vec<(String, String)>) {
    for (id, name) in pending.drain(..) {
        out.push(missing_tool_result(&id, &name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assistant_text, tool_result};
    use pipi_protocol::{ContentBlock, StopReason, Usage};

    #[test]
    fn estimation_heuristic() {
        // 4 字符 ≈ 1 token
        assert_eq!(estimate_tokens(&Message::user_text("abcdefgh")), 2);
        // 图片按 4800 chars 计
        let image = Message::ToolResult {
            tool_call_id: "t".into(),
            tool_name: "read".into(),
            content: vec![ToolResultContent::Image {
                data: "x".into(),
                mime_type: "image/png".into(),
            }],
            is_error: false,
            details: None,
            timestamp: 0,
        };
        assert_eq!(estimate_tokens(&image), ESTIMATED_IMAGE_CHARS as u64 / 4);
    }

    #[test]
    fn prune_cuts_at_user_boundaries_only() {
        // user(长) → assistant → toolResult → user(短) → assistant(短)
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text(&"y".repeat(100)),
            tool_result("t1", "bash", &"z".repeat(100)),
            Message::user_text("short question"),
            assistant_text("answer"),
        ];
        // 预算只够放下后两条
        let cut = prune_cut_index(&messages, 900).expect("超预算应给切点");
        assert_eq!(cut, 3);
        assert_eq!(messages[cut].role(), "user");
        assert_eq!(messages.len() - cut, 2);
        // 预算充足：不动
        assert_eq!(prune_cut_index(&messages, 99_900), None);
    }

    #[test]
    fn prune_never_orphans_tool_results() {
        // 首条就是 assistant + toolResult（无 user 边界在尾部预算内）
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text(&"y".repeat(10_000)),
            tool_result("t1", "bash", &"z".repeat(10_000)),
        ];
        assert_eq!(prune_cut_index(&messages, 500), None);
    }

    fn assistant_with_calls(calls: &[(&str, &str)]) -> Message {
        Message::Assistant {
            content: calls
                .iter()
                .map(|(id, name)| ContentBlock::ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: serde_json::json!({}),
                })
                .collect(),
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    fn role_ids(messages: &[Message]) -> Vec<(String, String)> {
        messages
            .iter()
            .map(|message| match message {
                Message::User { .. } => ("user".into(), String::new()),
                Message::Assistant { .. } => ("assistant".into(), String::new()),
                Message::ToolResult {
                    tool_call_id,
                    is_error,
                    ..
                } => (
                    if *is_error { "tool-error" } else { "tool" }.into(),
                    tool_call_id.clone(),
                ),
            })
            .collect()
    }

    #[test]
    fn healthy_history_is_unchanged() {
        let messages = vec![
            Message::user_text("hi"),
            assistant_with_calls(&[("c1", "bash"), ("c2", "read")]),
            tool_result("c1", "bash", "out"),
            tool_result("c2", "read", "out"),
            Message::assistant_text("done", "m"),
        ];
        let repaired = repair_tool_pairing(messages.clone());
        assert_eq!(repaired, messages);
    }

    #[test]
    fn missing_results_are_synthesized_right_after_the_assistant() {
        let messages = vec![
            Message::user_text("hi"),
            assistant_with_calls(&[("c1", "bash")]),
            Message::user_text("继续"),
            Message::assistant_text("ok", "m"),
        ];
        let repaired = repair_tool_pairing(messages);
        assert_eq!(
            role_ids(&repaired),
            vec![
                ("user".to_string(), String::new()),
                ("assistant".to_string(), String::new()),
                ("tool-error".to_string(), "c1".to_string()),
                ("user".to_string(), String::new()),
                ("assistant".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn partial_batch_is_completed_in_order() {
        let messages = vec![
            assistant_with_calls(&[("c1", "bash"), ("c2", "read"), ("c3", "grep")]),
            tool_result("c1", "bash", "out"),
        ];
        let repaired = repair_tool_pairing(messages);
        assert_eq!(
            role_ids(&repaired),
            vec![
                ("assistant".to_string(), String::new()),
                ("tool".to_string(), "c1".to_string()),
                ("tool-error".to_string(), "c2".to_string()),
                ("tool-error".to_string(), "c3".to_string()),
            ]
        );
    }

    #[test]
    fn orphan_tool_results_are_dropped() {
        let messages = vec![
            Message::user_text("hi"),
            tool_result("ghost", "bash", "out"),
            assistant_with_calls(&[("c1", "bash")]),
            tool_result("c1", "bash", "out"),
        ];
        let repaired = repair_tool_pairing(messages);
        assert_eq!(
            role_ids(&repaired),
            vec![
                ("user".to_string(), String::new()),
                ("assistant".to_string(), String::new()),
                ("tool".to_string(), "c1".to_string()),
            ]
        );
    }

    #[test]
    fn histories_without_tools_pass_through() {
        let messages = vec![
            Message::user_text("a"),
            Message::assistant_text("b", "m"),
            Message::user_text("c"),
        ];
        assert_eq!(repair_tool_pairing(messages.clone()), messages);
    }

    fn assistant_with_usage(text: &str, total: u64, timestamp: u64, stop: StopReason) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::Text { text: text.into() }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Usage {
                total_tokens: total,
                ..Default::default()
            },
            stop_reason: stop,
            error_message: None,
            timestamp,
            duration_ms: None,
        }
    }

    fn user_at(text: &str, timestamp: u64) -> Message {
        Message::User {
            content: text.into(),
            timestamp,
        }
    }

    #[test]
    fn anchor_is_last_valid_assistant_usage() {
        let messages = vec![
            user_at("q", 1),
            assistant_with_usage("a", 5_000, 2, StopReason::Stop),
            user_at("q2", 3),
            assistant_with_usage("bad", 9_999, 4, StopReason::Error),
        ];
        // error 轮次的用量不计：锚点仍是 index 1
        assert_eq!(last_usage_anchor(&messages), Some((1, 5_000)));
    }

    #[test]
    fn anchor_ignores_zero_usage_and_falls_back_to_estimate() {
        let messages = vec![
            user_at("hello", 1),
            assistant_with_usage("x", 0, 2, StopReason::Stop),
        ];
        assert_eq!(last_usage_anchor(&messages), None);
        assert_eq!(
            context_tokens(&messages),
            estimate_context_tokens(&messages)
        );
    }

    #[test]
    fn context_tokens_adds_trailing_estimate_to_anchor() {
        let trailing = user_at(&"x".repeat(400), 3);
        let messages = vec![
            user_at("q", 1),
            assistant_with_usage("a", 5_000, 2, StopReason::Stop),
            trailing.clone(),
        ];
        assert_eq!(
            context_tokens(&messages),
            5_000 + estimate_tokens(&trailing)
        );
    }

    #[test]
    fn anchor_is_invalidated_by_newer_prefix_message() {
        // 压缩摘要（时间戳晚于旧助手轮次）插在前面：旧 usage 描述的是旧前缀，不再适用
        let messages = vec![
            user_at("summary", 100),
            assistant_with_usage("old", 5_000, 50, StopReason::Stop),
        ];
        assert_eq!(last_usage_anchor(&messages), None);
        assert_eq!(
            context_tokens(&messages),
            estimate_context_tokens(&messages)
        );
    }
}
