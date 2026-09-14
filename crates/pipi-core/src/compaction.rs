//! LLM 摘要式上下文压缩。
//!
//! 移植自 `packages/agent/src/harness/compaction/compaction.ts` 的核心思路：
//! 上下文超预算时，让同一个模型把旧轮次压成一条摘要，替换进消息历史，
//! 并保留最近的完整轮次原文（切点只落在 user 消息边界，绝不孤立
//! toolResult —— 与 [`crate::context::prune_oldest`] 同一不变量）。
//!
//! 解耦约定：本模块只负责「怎么压」（纯函数 + 一次 LLM 调用）；「何时压」
//! 与「压缩结果如何持久化」由 runtime 决定。摘要调用复用 provider 的流式
//! 路径（空 tools 即纯文本请求），无独立的非流式 API。

use crate::context::{estimate_context_tokens, should_compact, DEFAULT_RESERVE_TOKENS};
use crate::provider::Provider;
use crate::types::{AbortSignal, Context, Message, Model, StreamEvent, StreamOptions};

/// 摘要正文的 token 上限（约 4000 字；防摘要失控，无需等于 reserve 全额）。
const SUMMARY_MAX_TOKENS: u32 = 1024;

/// 保留近期原文轮次的 token 预算（对齐 pi 的 keepRecentTokens 默认值）。
/// 不超过该值的原文轮次按原文保留，其余并入摘要。
const KEEP_RECENT_TOKENS: u64 = 20_000;

/// 压缩结果：摘要 + 保留的近期轮次。
#[derive(Debug, Clone)]
pub struct Compacted {
    /// 压缩后的完整消息历史（首条为摘要 User 消息，其后是保留的原文轮次）。
    pub messages: Vec<Message>,
    /// 被摘要替换掉的消息条数。
    pub replaced: usize,
}

/// 序列化一条消息为供模型阅读的紧凑文本（含 role 前缀与工具信息）。
fn render_message(message: &Message) -> String {
    match message {
        Message::User { content, .. } => format!("[user]\n{content}"),
        Message::Assistant { content, .. } => {
            let mut parts = Vec::new();
            for block in content {
                match block {
                    crate::types::ContentBlock::Text { text } => {
                        parts.push(format!("[assistant]\n{text}"))
                    }
                    crate::types::ContentBlock::Thinking { .. } => {}
                    crate::types::ContentBlock::ToolCall {
                        name, arguments, ..
                    } => parts.push(format!(
                        "[assistant tool call: {name}] {}",
                        serde_json::to_string(arguments).unwrap_or_default()
                    )),
                }
            }
            if parts.is_empty() {
                "[assistant] (empty)".into()
            } else {
                parts.join("\n")
            }
        }
        Message::ToolResult {
            tool_name,
            content,
            is_error,
            ..
        } => {
            let body: String = content
                .iter()
                .map(|c| match c {
                    crate::types::ToolResultContent::Text { text } => text.clone(),
                    crate::types::ToolResultContent::Image { .. } => "[image]".into(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            let tag = if *is_error { "error" } else { "ok" };
            format!("[tool result: {tool_name} ({tag})]\n{body}")
        }
    }
}

/// 找到保留尾部的切点：从尾部向前累计预算，切点只落在 user 消息边界
/// （`prune_oldest` 的镜像逻辑：前者从头找能保留的尾，这里从尾找该丢的头）。
/// 返回 `None` 表示整段历史都在预算内（无需压缩）；返回 `Some(len)` 表示
/// 尾部无可行 user 边界，保留空段（全部摘要化，由摘要自身兜底）。
fn find_keep_start(messages: &[Message], budget_tokens: u64) -> Option<usize> {
    let mut tail = 0u64;
    for i in (0..messages.len()).rev() {
        tail += crate::context::estimate_tokens(&messages[i]);
        if tail > budget_tokens {
            // messages[i] 放不下：保留段必须从它之后、最近的 user 边界开始
            return Some(
                (i + 1..messages.len())
                    .find(|&j| messages[j].role() == "user")
                    .unwrap_or(messages.len()),
            );
        }
    }
    None // 全部都在预算内：无需保留切点（也不会触发压缩）
}

/// 构造摘要请求：固定骨架（对齐 pi 的 compaction prompt），避免重复压缩
/// 时因措辞漂移丢失信息。有前一份摘要时用 update 版指令，旧摘要放
/// `<previous-summary>` 并要求保留其中全部既有信息。
fn build_summary_prompt(messages: &[Message], previous_summary: Option<&str>) -> String {
    let transcript: String = messages
        .iter()
        .map(render_message)
        .collect::<Vec<_>>()
        .join("\n\n");
    let previous_section = previous_summary
        .map(|summary| format!("<previous-summary>\n{summary}\n</previous-summary>\n\n"))
        .unwrap_or_default();
    let instruction = if previous_summary.is_some() {
        "Update the previous summary with new information from the conversation below. \
PRESERVE all existing information from the previous summary, add new facts, \
and move items from \"In Progress\" to \"Done\" when completed. \
ONLY output the updated summary."
    } else {
        "Write a structured context checkpoint summary that another LLM will use \
to continue the work seamlessly. Use EXACTLY this skeleton:\n\n\
## Goal\n\n## Constraints & Preferences\n\n## Progress\n\n\
### Done\n\n### In Progress\n\n### Blocked\n\n## Key Decisions\n\n\
## Next Steps\n\n## Critical Context\n\n\
Keep each section concise. Preserve exact file paths, function names, and error \
messages. Write in the same language as the conversation."
    };
    format!("{previous_section}{instruction}\n\n<conversation>\n\n{transcript}\n\n</conversation>")
}

/// 摘要调用的系统提示：限定为总结者角色，禁止把对话接下去（对齐 pi）。
const SUMMARY_SYSTEM_PROMPT: &str = "You are a context summarization assistant. \
Do NOT continue the conversation. Do NOT respond to any questions in the conversation. \
ONLY output the structured summary.";

/// 把摘要包装成与 pi 一致的 User 消息。
fn summary_message(summary: &str) -> Message {
    Message::user_text(format!(
        "<compaction summary>\n{summary}\n</compaction summary>"
    ))
}

/// 提取消息中的纯文本（摘要调用只要文本块）。
fn message_text(message: &Message) -> String {
    match message {
        Message::Assistant { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                crate::types::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        Message::User { content, .. } => content.clone(),
        Message::ToolResult { .. } => String::new(),
    }
}

/// 执行一次摘要压缩。
///
/// `context_window` / `reserve_tokens` 决定「保留多少原文轮次」：摘要替换
/// 掉的部分 + 保留尾部合计仍受同一预算约束。摘要 LLM 调用可被 `abort`
/// 中断；调用失败时返回 Err，由上层决定是否回退到硬裁剪。
pub async fn compact(
    provider: &dyn Provider,
    model: &Model,
    options: &StreamOptions,
    messages: &[Message],
    context_window: u64,
    reserve_tokens: u64,
    abort: AbortSignal,
) -> Result<Compacted, String> {
    if messages.is_empty() {
        return Err("空上下文无需压缩".into());
    }

    // 保留尾部的预算：预留 + 摘要自身的空间（摘要约 SUMMARY_MAX_TOKENS），
    // 并收敛到 pi 的 keepRecentTokens 默认值以内
    let keep_budget = context_window
        .saturating_sub(reserve_tokens)
        .saturating_sub(SUMMARY_MAX_TOKENS as u64 * 2)
        .min(KEEP_RECENT_TOKENS);

    let keep_start = find_keep_start(messages, keep_budget).ok_or_else(|| {
        "历史无需压缩：尚未超出保留预算".to_string()
    })?;

    // 再次压缩时，历史首条是上次注入的摘要消息 —— 取出旧摘要交给
    // update 版指令，避免措辞漂移（pi 的 previousSummary 语义）。
    let previous_summary = match messages.first() {
        Some(Message::User { content, .. }) => content
            .strip_prefix("<compaction summary>\n")
            .and_then(|rest| rest.split("</compaction summary>").next())
            .map(str::trim)
            .filter(|summary| !summary.is_empty()),
        _ => None,
    };

    let to_summarize = &messages[..keep_start];
    let keep = &messages[keep_start..];

    // 摘要调用：空 tools 的纯文本请求，收集流式事件直到 Done
    let summary_context = Context {
        system_prompt: Some(SUMMARY_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user_text(build_summary_prompt(
            to_summarize,
            previous_summary,
        ))],
        tools: vec![],
    };
    let mut summary_options = options.clone();
    summary_options.max_tokens = Some(SUMMARY_MAX_TOKENS);

    let mut rx = provider
        .stream(model, &summary_context, &summary_options, abort.clone())
        .await;
    let mut summary_text = String::new();
    let mut stream_error: Option<String> = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { delta, .. } => summary_text.push_str(&delta),
            StreamEvent::Done { message, .. } => {
                // 以最终消息为准（Thinking 等非文本块被排除）
                let text = message_text(&message);
                if !text.trim().is_empty() {
                    summary_text = text;
                }
            }
            StreamEvent::Error { message } => {
                stream_error = Some(message);
                break;
            }
            _ => {}
        }
        if abort.is_aborted() {
            return Err("摘要调用已中止".into());
        }
    }
    if abort.is_aborted() {
        return Err("摘要调用已中止".into());
    }
    if let Some(error) = stream_error {
        return Err(format!("摘要调用失败：{error}"));
    }
    if summary_text.trim().is_empty() {
        return Err("摘要调用失败：模型未返回摘要内容".into());
    }

    // 摘要末尾追加文件操作清单（对齐 pi 的 formatFileOperations）：
    // 继续工作时最需要知道动过哪些文件。
    let (read_files, modified_files) = file_operations(to_summarize);
    let mut summary = summary_text.trim().to_string();
    if !read_files.is_empty() {
        summary.push_str(&format!("\n\nFiles read: {}", read_files.join(", ")));
    }
    if !modified_files.is_empty() {
        summary.push_str(&format!(
            "\n\nFiles modified: {}",
            modified_files.join(", ")
        ));
    }

    let mut compacted = Vec::with_capacity(keep.len() + 1);
    compacted.push(summary_message(&summary));
    compacted.extend(keep.iter().cloned());
    Ok(Compacted {
        messages: compacted,
        replaced: keep_start,
    })
}

/// 从被摘要的消息中提取文件操作（read → 读取清单，write/edit → 修改清单）。
/// glob 的 `path` 参数是搜索根而非文件，bash 参数不可靠，都跳过。
fn file_operations(messages: &[Message]) -> (Vec<String>, Vec<String>) {
    use std::collections::BTreeSet;
    let mut read = BTreeSet::new();
    let mut modified = BTreeSet::new();
    for message in messages {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        for block in content {
            let crate::types::ContentBlock::ToolCall {
                name, arguments, ..
            } = block
            else {
                continue;
            };
            let Some(path) = arguments["path"].as_str() else {
                continue;
            };
            match name.as_str() {
                "read" => {
                    read.insert(path.to_string());
                }
                "write" | "edit" => {
                    modified.insert(path.to_string());
                }
                _ => {}
            }
        }
    }
    (read.into_iter().collect(), modified.into_iter().collect())
}

/// 压缩决策：上下文是否超预算（供 runtime 在 turn 边界检查）。
pub fn needs_compaction(messages: &[Message], context_window: u64) -> bool {
    if context_window == 0 {
        return false;
    }
    should_compact(
        estimate_context_tokens(messages),
        context_window,
        DEFAULT_RESERVE_TOKENS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::estimate_tokens;
    use crate::types::StopReason;

    fn assistant_text(text: &str) -> Message {
        Message::Assistant {
            content: vec![crate::types::ContentBlock::Text { text: text.into() }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
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
            content: vec![crate::types::ToolResultContent::Text { text: text.into() }],
            is_error: false,
            details: None,
            timestamp: 0,
        }
    }

    #[test]
    fn renders_all_message_kinds() {
        let transcript = build_summary_prompt(
            &[
                Message::user_text("fix the bug"),
                assistant_text("let me check"),
                tool_result("ok"),
            ],
            None,
        );
        assert!(transcript.contains("[user]"));
        assert!(transcript.contains("fix the bug"));
        assert!(transcript.contains("[assistant]"));
        assert!(transcript.contains("[assistant tool call:") || transcript.contains("let me check"));
        assert!(transcript.contains("[tool result: bash (ok)]"));
        assert!(transcript.contains("<conversation>"));
        assert!(transcript.contains("## Goal"));
        assert!(transcript.contains("## Critical Context"));
        // 无前摘要时不含 previous-summary 节
        assert!(!transcript.contains("<previous-summary>"));
    }

    #[test]
    fn summary_prompt_includes_previous_summary_and_update_instruction() {
        let transcript = build_summary_prompt(&[Message::user_text("next turn")], Some("## Goal\n旧摘要"));
        assert!(transcript.contains("<previous-summary>\n## Goal\n旧摘要\n</previous-summary>"));
        assert!(transcript.contains("PRESERVE all existing information"));
        assert!(!transcript.contains("Use EXACTLY this skeleton"));
    }

    #[test]
    fn file_operations_extracts_read_and_modified_paths() {
        let tool_call = |name: &str, path: &str| Message::Assistant {
            content: vec![crate::types::ContentBlock::ToolCall {
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
        };
        let messages = vec![
            Message::user_text("go"),
            tool_call("read", "b.rs"),
            tool_call("read", "a.rs"),
            tool_call("write", "a.rs"),
            tool_call("edit", "c.rs"),
            tool_call("bash", "irrelevant"),
            tool_result("done"),
        ];
        let (read, modified) = file_operations(&messages);
        assert_eq!(read, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(modified, vec!["a.rs".to_string(), "c.rs".to_string()]);
    }

    #[test]
    fn keep_start_cuts_at_user_boundary_only() {
        // user(长) → assistant → toolResult → user(短) → assistant(短)
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text(&"y".repeat(100)),
            tool_result(&"z".repeat(100)),
            Message::user_text("recent question"),
            assistant_text("recent answer"),
        ];
        // 预算只够最近两条：切点必须落在 index 3（user 边界）
        let start = find_keep_start(&messages, 200).unwrap();
        assert_eq!(start, 3);
        assert_eq!(messages[start].role(), "user");
    }

    #[test]
    fn keep_start_never_orphans_tool_results() {
        // 尾部紧邻 toolResult、更早处无 user 边界时：保留空段（全部摘要化），
        // 绝不孤立 toolResult
        let messages = vec![
            Message::user_text("x".repeat(10_000)),
            assistant_text("a"),
            tool_result("r"),
        ];
        let start = find_keep_start(&messages, 100).unwrap();
        assert_eq!(start, messages.len());
    }

    #[test]
    fn keep_start_none_when_all_fit() {
        let messages = vec![Message::user_text("hello"), assistant_text("hi")];
        assert!(find_keep_start(&messages, 10_000).is_none());
    }

    #[test]
    fn needs_compaction_respects_zero_window() {
        assert!(!needs_compaction(&[Message::user_text("hi")], 0));
        assert!(needs_compaction(
            &[Message::user_text("x".repeat(100_000))],
            10_000
        ));
        assert_eq!(estimate_tokens(&Message::user_text("abcdefgh")), 2);
    }
}
