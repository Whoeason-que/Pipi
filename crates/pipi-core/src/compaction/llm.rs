//! 摘要替换策略（替换式）：让同一个模型把旧轮次压成一条摘要。
//!
//! 从原 `compaction.rs` 整体搬来，行为不变：切点只落 user 边界、固定骨架
//! prompt、重复压缩时把旧摘要交回（update 版指令）、摘要末尾追加文件操作
//! 清单。新增的是「产出 [`Plan`] 而不是直接改历史」，以及把摘要调用的
//! usage 带回上层（计入会话账本）。

use super::{file_operations, Plan, Preparation, Replacement, StrategyEnv};
use crate::types::{AbortSignal, Context, Message, StreamEvent, StreamOptions, Usage};

/// 摘要调用的系统提示：限定为总结者角色，禁止把对话接下去（对齐 pi）。
const SUMMARY_SYSTEM_PROMPT: &str = "You are a context summarization assistant. \
Do NOT continue the conversation. Do NOT respond to any questions in the conversation. \
ONLY output the structured summary.";

pub struct LlmSummarize;

impl Replacement for LlmSummarize {
    fn name(&self) -> &'static str {
        "llm-summarize"
    }

    fn applies(&self, prep: &Preparation<'_>) -> bool {
        // 阈值由 runtime 判（自动看 needs_compaction、手动不看）；
        // 这里只回答「有没有东西可压」——`run` 里切不出旧轮次时会给出可读错误。
        !prep.messages.is_empty()
    }

    fn run<'a>(
        &'a self,
        prep: &'a Preparation<'a>,
        env: &'a StrategyEnv<'a>,
    ) -> super::BoxFuture<'a, Result<Plan, String>> {
        Box::pin(async move {
            // 保留尾部的切点（尾部无可行 user 边界时保留空段，全部摘要化）
            let keep_start = find_keep_start(prep.messages, prep.budget.keep_budget())
                .ok_or_else(|| "历史无需压缩：尚未超出保留预算".to_string())?;
            let to_summarize = &prep.messages[..keep_start];

            let (summary_text, usage) =
                summarize(to_summarize, prep.previous_summary(), env).await?;

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

            Ok(Plan {
                keep_from: Some(keep_start),
                injections: vec![crate::session::summary_message(&summary)],
                usage,
                ..Default::default()
            })
        })
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

/// 发一次摘要请求（空 tools 的纯文本请求，复用 provider 流式路径），
/// 失败按 [`crate::retry`] 重发。返回（摘要正文, 本次调用的用量）。
///
/// 与对话路径的差别：摘要文本攒在本地、没有任何增量发给用户，所以**重放永远安全**
/// ——不适用「已输出正文不重试」那条规则。
async fn summarize(
    messages: &[Message],
    previous_summary: Option<&str>,
    env: &StrategyEnv<'_>,
) -> Result<(String, Option<Usage>), String> {
    let context = Context {
        system_prompt: Some(SUMMARY_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user_text(build_summary_prompt(
            messages,
            previous_summary,
        ))],
        tools: vec![],
    };
    let mut options = env.options.clone();
    options.max_tokens = Some(env.model.max_tokens.min(super::SUMMARY_MAX_TOKENS));

    let mut failures: u32 = 0;
    loop {
        match summarize_once(env, &context, &options).await {
            Ok(value) => return Ok(value),
            Err(SummarizeFailure::Fatal(message)) => return Err(message),
            Err(SummarizeFailure::Retryable { message, hint }) => {
                failures += 1;
                match crate::retry::retry_decision(&env.retry, failures, hint) {
                    crate::retry::RetryDecision::Retry { delay } => {
                        tokio::select! {
                            _ = env.abort.wait_aborted() => {
                                return Err("摘要调用已中止".into());
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                    crate::retry::RetryDecision::GiveUp(reason) => {
                        return Err(summarize_give_up(&message, failures, reason));
                    }
                }
            }
        }
    }
}

/// 单次摘要尝试的失败：终态 或 可重试（带等待提示）。
enum SummarizeFailure {
    Fatal(String),
    Retryable {
        message: String,
        hint: Option<crate::retry::RetryHint>,
    },
}

/// 放弃重试后的文案。
fn summarize_give_up(
    cause: &str,
    failures: u32,
    reason: crate::retry::GiveUpReason,
) -> String {
    match reason {
        crate::retry::GiveUpReason::PolicyExhausted => {
            format!("摘要调用失败（已重试 {failures} 次）：{cause}")
        }
        crate::retry::GiveUpReason::TimeoutBudget => {
            format!("摘要调用连续超时（{failures} 次）：{cause}")
        }
        crate::retry::GiveUpReason::ServerDelayTooLong(after_ms) => format!(
            "摘要调用失败：服务端要求 {} 秒后再试，超过重试上限：{cause}",
            after_ms / 1_000
        ),
    }
}

/// 单次尝试：发请求并把文本攒起来。
async fn summarize_once(
    env: &StrategyEnv<'_>,
    context: &Context,
    options: &StreamOptions,
) -> Result<(String, Option<Usage>), SummarizeFailure> {
    let abort: AbortSignal = env.abort.clone();
    let mut rx = env
        .provider
        .stream(env.model, context, options, env.abort.clone())
        .await;
    let mut summary_text = String::new();
    let mut usage: Option<Usage> = None;
    let mut failure: Option<SummarizeFailure> = None;
    // 流必须走到终态事件才算成功：干净 EOF 会把半截摘要当成功返回（曾经的缺口）
    let mut saw_done = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { delta, .. } => summary_text.push_str(&delta),
            StreamEvent::Done {
                message,
                usage: done_usage,
                ..
            } => {
                usage = Some(done_usage);
                // 以最终消息为准（Thinking 等非文本块被排除）
                let text = message_text(&message);
                if !text.trim().is_empty() {
                    summary_text = text;
                }
                saw_done = true;
            }
            StreamEvent::Error { message, retry } => {
                failure = Some(match retry {
                    Some(hint) => SummarizeFailure::Retryable {
                        message,
                        hint: Some(hint),
                    },
                    None => SummarizeFailure::Fatal(format!("摘要调用失败：{message}")),
                });
                break;
            }
            _ => {}
        }
        if abort.is_aborted() {
            return Err(SummarizeFailure::Fatal("摘要调用已中止".into()));
        }
    }
    if abort.is_aborted() {
        return Err(SummarizeFailure::Fatal("摘要调用已中止".into()));
    }
    if let Some(failure) = failure {
        return Err(failure);
    }
    if !saw_done {
        return Err(SummarizeFailure::Retryable {
            message: "流提前结束：未收到结束帧".into(),
            hint: Some(crate::retry::RetryHint::plain()),
        });
    }
    if summary_text.trim().is_empty() {
        return Err(SummarizeFailure::Fatal(
            "摘要调用失败：模型未返回摘要内容".into(),
        ));
    }
    Ok((summary_text, usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::tests::{assistant_text, tool_result};

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
}
