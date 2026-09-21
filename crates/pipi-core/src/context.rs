//! 上下文预算：token 估算、裁剪与环境上下文。
//!
//! - `estimate_tokens` / `prune_oldest` 移植自
//!   `packages/agent/src/harness/compaction/compaction.ts` 的启发式部分
//!   （chars/4、图片按 4800 chars 计）。上游的完整 compaction 用 LLM 做
//!   摘要替换，属「有意推迟」；这里是 M1 的保底策略：超预算时从最旧的
//!   完整轮次开始丢弃。
//! - `environment_context` 移植自 codex
//!   `core/src/context/environment_context.rs` 的思路：把工作目录、沙箱
//!   等运行环境以 XML 标签块注入上下文（上游作为独立 user fragment 注入，
//!   我们拼进系统提示，见 agents::build_system_prompt）。

use std::path::Path;

use crate::permissions::SandboxMode;
use crate::types::{ContentBlock, Message, ToolResultContent};

/// 为 compaction 预留的 token（经验值，对齐 pi 的预留思路）。
pub const DEFAULT_RESERVE_TOKENS: u64 = 16_384;

/// 图片内容块的估算字符数（pi 的 ESTIMATED_IMAGE_CHARS）。
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// 估算单条消息的 token（pi 的 chars/4 保守启发式）。
pub fn estimate_tokens(message: &Message) -> u64 {
    let chars: usize = match message {
        Message::User { content, .. } => content.len(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::Thinking { thinking, .. } => thinking.len(),
                ContentBlock::ToolCall {
                    name, arguments, ..
                } => name.len() + serde_json::to_string(arguments).unwrap_or_default().len(),
            })
            .sum(),
        Message::ToolResult { content, .. } => content
            .iter()
            .map(|c| match c {
                ToolResultContent::Text { text } => text.len(),
                ToolResultContent::Image { .. } => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
    };
    chars.div_ceil(4) as u64
}

/// 估算整段消息历史的 token。
pub fn estimate_context_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_tokens).sum()
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
/// 所以决策与执行分开：`prune_oldest` 也走这里，规则只有一份。
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

/// 保底裁剪：从最旧的**完整轮次**开始丢弃，直到尾部预算 ≤ 上限。
/// 返回 (裁剪后消息, 丢弃条数)。切点规则见 [`prune_cut_index`]。
pub fn prune_oldest(messages: &[Message], budget_tokens: u64) -> (Vec<Message>, usize) {
    match prune_cut_index(messages, budget_tokens) {
        Some(i) => (messages[i..].to_vec(), i),
        None => (messages.to_vec(), 0),
    }
}

/// 生成供 loop 用的裁剪 transform（pi 的 transformContext 钩子的默认实现）。
pub fn prune_transform(budget_tokens: u64) -> impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync {
    move |messages| prune_oldest(&messages, budget_tokens).0
}

/// 结果缺失时写给模型看的合成 tool 结果文案（发送前修复与载入自愈共用）。
pub const MISSING_TOOL_RESULT_TEXT: &str =
    "工具结果缺失：这一轮执行被中断（应用重启 / 强制停止），该调用没有产出结果。";

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
        timestamp: crate::types::now_millis(),
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

/// 将文件系统路径安全地渲染进 prompt：规范化分隔符、可见化控制字符并
/// 转义 XML 特殊字符，避免路径破坏上下文标签或注入额外行。
pub(crate) fn escape_path_for_prompt(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let mut sanitized = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        match character {
            '\n' => sanitized.push_str("\\n"),
            '\r' => sanitized.push_str("\\r"),
            '\t' => sanitized.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(sanitized, "\\u{{{:x}}}", character as u32).unwrap();
            }
            character => sanitized.push(character),
        }
    }
    sanitized
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 运行环境上下文块（codex 思路，简化渲染）。
pub fn environment_context(workspace: &Path, sandbox: &SandboxMode) -> String {
    let cwd = escape_path_for_prompt(&workspace.display().to_string());
    format!(
        "<environment_context>\n<cwd>{cwd}</cwd>\n<sandbox>{}</sandbox>\n<platform>{}</platform>\n<date>{}</date>\n</environment_context>",
        sandbox.as_str(),
        std::env::consts::OS,
        today(),
    )
}

/// ISO 日期（YYYY-MM-DD），无 chrono 依赖的 civil 算法。
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    // Howard Hinnant 的 civil_from_days
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StopReason, Usage};

    fn assistant_text(text: &str) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::Text { text: text.into() }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    fn tool_result(text: &str) -> Message {
        Message::ToolResult {
            tool_call_id: "t1".into(),
            tool_name: "bash".into(),
            content: vec![ToolResultContent::Text { text: text.into() }],
            is_error: false,
            details: None,
            timestamp: 0,
        }
    }

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
            tool_result(&"z".repeat(100)),
            Message::user_text("short question"),
            assistant_text("answer"),
        ];
        // 预算只够放下后两条
        let (pruned, dropped) = prune_oldest(&messages, 900);
        assert_eq!(dropped, 3);
        assert_eq!(pruned.len(), 2);
        assert_eq!(pruned[0].role(), "user");
        // 预算充足：不动
        let (pruned, dropped) = prune_oldest(&messages, 99_900);
        assert_eq!((pruned.len(), dropped), (5, 0));
    }

    #[test]
    fn prune_never_orphans_tool_results() {
        // 首条就是 assistant + toolResult（无 user 边界在尾部预算内）
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text(&"y".repeat(10_000)),
            tool_result(&"z".repeat(10_000)),
        ];
        let (pruned, dropped) = prune_oldest(&messages, 500);
        assert_eq!((pruned.len(), dropped), (3, 0));
    }

    #[test]
    fn env_context_renders_tags() {
        let ctx = environment_context(Path::new("/home/u/proj"), &SandboxMode::WorkspaceWrite);
        assert!(ctx.contains("<environment_context>"));
        assert!(ctx.contains("<cwd>/home/u/proj</cwd>"));
        assert!(ctx.contains("<sandbox>workspace-write</sandbox>"));
        assert!(ctx.contains("<date>20"));
    }

    #[test]
    fn env_context_escapes_hostile_workspace_path() {
        let ctx = environment_context(
            Path::new("/tmp/a<&>\"'\nnext"),
            &SandboxMode::WorkspaceWrite,
        );
        assert!(ctx.contains("<cwd>/tmp/a&lt;&amp;&gt;&quot;&apos;\\nnext</cwd>"));
        assert!(!ctx.contains("<cwd>/tmp/a<&>\"'"));
    }
}

#[cfg(test)]
mod pairing_tests {
    use super::*;
    use crate::types::{ContentBlock, StopReason, Usage};

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

    fn tool_result(id: &str, name: &str) -> Message {
        Message::ToolResult {
            tool_call_id: id.into(),
            tool_name: name.into(),
            content: vec![ToolResultContent::Text { text: "out".into() }],
            is_error: false,
            details: None,
            timestamp: 0,
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
            tool_result("c1", "bash"),
            tool_result("c2", "read"),
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
            tool_result("c1", "bash"),
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
            tool_result("ghost", "bash"),
            assistant_with_calls(&[("c1", "bash")]),
            tool_result("c1", "bash"),
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
}
