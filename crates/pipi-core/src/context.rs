//! 上下文预算：token 估算、裁剪与环境上下文。
//!
//! - `estimate_tokens` / `should_compact` / `prune_oldest` 移植自
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

/// 上下文是否超出 compaction 阈值（pi 的 shouldCompact）。
pub fn should_compact(context_tokens: u64, context_window: u64, reserve_tokens: u64) -> bool {
    context_tokens > context_window.saturating_sub(reserve_tokens)
}

/// 保底裁剪：从最旧的**完整轮次**开始丢弃，直到尾部预算 ≤ 上限。
///
/// 切点只落在 user 消息的开头 —— toolResult 永远不会与其 assistant 分离
/// （provider 会拒绝孤儿 toolResult）。找不到可行切点（尾部本身就超预算）
/// 时原样返回，交由上层处理（provider 会以 length 停止，loop 已有对应
/// 处理路径）。返回 (裁剪后消息, 丢弃条数)。
pub fn prune_oldest(
    messages: &[Message],
    context_window: u64,
    reserve_tokens: u64,
) -> (Vec<Message>, usize) {
    let budget = context_window.saturating_sub(reserve_tokens);
    let total = estimate_context_tokens(messages);
    if total <= budget || messages.is_empty() {
        return (messages.to_vec(), 0);
    }

    // 前缀累计 token
    let mut cum = 0u64;
    let mut cut: Option<usize> = None;
    for (i, message) in messages.iter().enumerate() {
        if message.role() == "user" && i > 0 {
            let tail = total - cum;
            if tail <= budget {
                cut = Some(i);
                break;
            }
        }
        cum += estimate_tokens(message);
    }

    match cut {
        Some(i) => (messages[i..].to_vec(), i),
        None => (messages.to_vec(), 0),
    }
}

/// 生成供 loop 用的裁剪 transform（pi 的 transformContext 钩子的默认实现）。
pub fn prune_transform(
    context_window: u64,
    reserve_tokens: u64,
) -> impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync {
    move |messages| prune_oldest(&messages, context_window, reserve_tokens).0
}

/// 运行环境上下文块（codex 思路，简化渲染）。
pub fn environment_context(workspace: &Path, sandbox: &SandboxMode) -> String {
    format!(
        "<environment_context>\n<cwd>{}</cwd>\n<sandbox>{}</sandbox>\n<platform>{}</platform>\n<date>{}</date>\n</environment_context>",
        workspace.display(),
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
        let (pruned, dropped) = prune_oldest(&messages, 1000, 100);
        assert_eq!(dropped, 3);
        assert_eq!(pruned.len(), 2);
        assert_eq!(pruned[0].role(), "user");
        // 预算充足：不动
        let (pruned, dropped) = prune_oldest(&messages, 100_000, 100);
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
        let (pruned, dropped) = prune_oldest(&messages, 500, 0);
        assert_eq!((pruned.len(), dropped), (3, 0));
    }

    #[test]
    fn compact_threshold() {
        assert!(should_compact(90_000, 100_000, 16_384));
        assert!(!should_compact(50_000, 100_000, 16_384));
    }

    #[test]
    fn env_context_renders_tags() {
        let ctx = environment_context(Path::new("/home/u/proj"), &SandboxMode::WorkspaceWrite);
        assert!(ctx.contains("<environment_context>"));
        assert!(ctx.contains("<cwd>/home/u/proj</cwd>"));
        assert!(ctx.contains("<sandbox>workspace-write</sandbox>"));
        assert!(ctx.contains("<date>20"));
    }
}
