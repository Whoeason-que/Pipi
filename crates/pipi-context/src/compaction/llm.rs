//! 摘要替换策略（替换式）：让同一个模型把旧轮次压成一条摘要。
//!
//! 从原 `compaction.rs` 整体搬来，行为不变：切点只落 user 边界、固定骨架
//! prompt、重复压缩时把旧摘要交回（update 版指令）、摘要末尾追加文件操作
//! 清单。新增的是「产出 [`Plan`] 而不是直接改历史」，以及把摘要调用的
//! usage 带回上层（计入会话账本）。

use super::{MIN_SUMMARY_MAX_TOKENS, Plan, Preparation, StrategyEnv, file_operations};
use pipi_protocol::{AbortSignal, Context, Message, StopReason, StreamEvent, StreamOptions, Usage};

/// 摘要调用的系统提示：限定为总结者角色，禁止把对话接下去（对齐 pi）。
const SUMMARY_SYSTEM_PROMPT: &str = "You are a context summarization assistant. \
Do NOT continue the conversation. Do NOT respond to any questions in the conversation. \
ONLY output the structured summary.";

pub struct LlmSummarize;

impl LlmSummarize {
    pub(super) const NAME: &'static str = "llm-summarize";

    /// 让同一个模型把旧轮次压成一条摘要。
    pub(super) async fn run(
        &self,
        prep: &Preparation<'_>,
        env: &StrategyEnv<'_>,
    ) -> Result<Plan, String> {
        // 保留尾部的切点（尾部无可行 user 边界时保留空段，全部摘要化）
        let keep_start = find_keep_start(prep.messages, prep.budget.keep_budget_for(prep.tokens()))
            .ok_or_else(|| "历史无需压缩：尚未超出保留预算".to_string())?;
        let to_summarize = &prep.messages[..keep_start];

        let (summary_text, usage) = summarize(
            to_summarize,
            prep.previous_summary(),
            prep.budget.summary_max_for(prep.tokens()),
            env,
        )
        .await?;

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
            injections: vec![pipi_protocol::session::summary_message(&summary)],
            usage,
            ..Default::default()
        })
    }
}

/// 找到保留尾部的切点：从尾部向前累计预算，切点只落在 user 消息边界
/// （`prune_cut_index` 的镜像逻辑：前者从头找能保留的尾，这里从尾找该丢的头）。
/// 返回 `None` 表示整段历史都在预算内（无需压缩）；返回 `Some(len)` 表示
/// 尾部无可行 user 边界，保留空段（全部摘要化，由摘要自身兜底）。
pub(super) fn find_keep_start(messages: &[Message], budget_tokens: u64) -> Option<usize> {
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
        Message::User { content, .. } => format!("[user]\n{}", content.text()),
        Message::Assistant { content, .. } => {
            let mut parts = Vec::new();
            for block in content {
                match block {
                    pipi_protocol::ContentBlock::Text { text } => {
                        parts.push(format!("[assistant]\n{text}"))
                    }
                    pipi_protocol::ContentBlock::Thinking { .. } => {}
                    pipi_protocol::ContentBlock::ToolCall {
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
                    pipi_protocol::ToolResultContent::Text { text } => text.clone(),
                    pipi_protocol::ToolResultContent::Image { .. } => "[image]".into(),
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
pub(super) fn build_summary_prompt(messages: &[Message], previous_summary: Option<&str>) -> String {
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
                pipi_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        Message::User { content, .. } => content.text(),
        Message::ToolResult { .. } => String::new(),
    }
}

/// 发一次摘要请求（空 tools 的纯文本请求，复用 provider 流式路径），
/// 失败按 [`crate::retry`] 重发。返回（摘要正文, 本次调用的用量）。
///
/// 与对话路径的差别：摘要文本攒在本地、没有任何增量发给用户，所以**重放永远安全**
/// ——不适用「已输出正文不重试」那条规则。
pub(super) async fn summarize(
    messages: &[Message],
    previous_summary: Option<&str>,
    summary_max: u32,
    env: &StrategyEnv<'_>,
) -> Result<(String, Option<Usage>), String> {
    let mut context = Context {
        system_prompt: Some(SUMMARY_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user_text(build_summary_prompt(
            messages,
            previous_summary,
        ))],
        tools: vec![],
    };
    let mut options = env.options.clone();
    // Budget 的摘要预算就是实际请求上限；model.max_tokens=0 表示未声明上限。
    let model_max = if env.model.max_tokens == 0 {
        u32::MAX
    } else {
        env.model.max_tokens
    };
    options.max_tokens = Some(model_max.min(summary_max));

    let mut failures: u32 = 0;
    let mut condensed = false;
    loop {
        match summarize_once(env, &context, &options, "摘要").await {
            Ok(value) => return Ok(value),
            Err(SummarizeFailure::Fatal(message)) => return Err(message),
            Err(SummarizeFailure::Truncated { text, usage }) => {
                // 摘要被输出上限截断：把截断文本本身压短再试一次。同样的输入 +
                // 同样的上限重发大概率还会撞墙，所以这次换输入与上限。
                if condensed || text.trim().is_empty() {
                    return Err(truncated_summary_error(summary_max, usage.as_ref()));
                }
                condensed = true;
                let target = (summary_max / 2).max(MIN_SUMMARY_MAX_TOKENS);
                context = condense_context(&text, target);
                options.max_tokens = Some(model_max.min(target));
            }
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
                        return Err(summarize_give_up(&message, failures, reason, "摘要"));
                    }
                }
            }
        }
    }
}

/// 一次性纯文本调用（摘要/编辑共用）：发请求、按 [`crate::retry`] 重发，
/// 返回文本与用量。输出撞长度上限时不重试 —— 调用方拿到的 JSON/摘要都不可用。
pub(super) async fn call_plain(
    env: &StrategyEnv<'_>,
    context: &Context,
    options: &StreamOptions,
    label: &str,
) -> Result<(String, Option<Usage>), String> {
    let mut failures: u32 = 0;
    loop {
        match summarize_once(env, context, options, label).await {
            Ok(value) => return Ok(value),
            Err(SummarizeFailure::Fatal(message)) => return Err(message),
            Err(SummarizeFailure::Truncated { usage, .. }) => {
                return Err(format!(
                    "{label}调用达到长度上限（实际输出 {} 个 token），未保存可能截断的结果",
                    usage.map_or(0, |usage| usage.output)
                ));
            }
            Err(SummarizeFailure::Retryable { message, hint }) => {
                failures += 1;
                match crate::retry::retry_decision(&env.retry, failures, hint) {
                    crate::retry::RetryDecision::Retry { delay } => {
                        tokio::select! {
                            _ = env.abort.wait_aborted() => {
                                return Err(format!("{label}调用已中止"));
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                    crate::retry::RetryDecision::GiveUp(reason) => {
                        return Err(summarize_give_up(&message, failures, reason, label));
                    }
                }
            }
        }
    }
}

/// 被截断的摘要：把截断文本作为输入，要求模型压成更短的完整摘要。
fn condense_context(truncated: &str, target_tokens: u32) -> Context {
    Context {
        system_prompt: Some(SUMMARY_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user_text(format!(
            "The previous summary was cut off at the output token limit. Rewrite the truncated summary below as a complete, self-contained summary in at most {target_tokens} tokens. Preserve the most important facts, decisions, file paths and error messages; drop redundancy.\n\n<truncated-summary>\n{}\n</truncated-summary>",
            truncated.trim()
        ))],
        tools: vec![],
    }
}

fn truncated_summary_error(summary_max: u32, usage: Option<&Usage>) -> String {
    format!(
        "摘要调用达到长度上限（请求最多 {} 个输出 token，实际输出 {} 个），压缩重试仍被截断，未保存可能截断的摘要",
        summary_max,
        usage.map_or(0, |usage| usage.output)
    )
}

/// 单次纯文本尝试的失败：终态、可重试（带等待提示）或输出被截断。
pub(super) enum SummarizeFailure {
    Fatal(String),
    Retryable {
        message: String,
        hint: Option<pipi_protocol::RetryHint>,
    },
    /// 输出撞到上限：调用方可以决定压短重试（摘要）或直接失败（编辑）。
    Truncated {
        text: String,
        usage: Option<Usage>,
    },
}

/// 放弃重试后的文案。
fn summarize_give_up(
    cause: &str,
    failures: u32,
    reason: crate::retry::GiveUpReason,
    label: &str,
) -> String {
    match reason {
        crate::retry::GiveUpReason::PolicyExhausted => {
            format!("{label}调用失败（已重试 {failures} 次）：{cause}")
        }
        crate::retry::GiveUpReason::TimeoutBudget => {
            format!("{label}调用连续超时（{failures} 次）：{cause}")
        }
        crate::retry::GiveUpReason::ServerDelayTooLong(after_ms) => format!(
            "{label}调用失败：服务端要求 {} 秒后再试，超过重试上限：{cause}",
            after_ms / 1_000
        ),
    }
}

/// 单次尝试：发请求并把文本攒起来。
async fn summarize_once(
    env: &StrategyEnv<'_>,
    context: &Context,
    options: &StreamOptions,
    label: &str,
) -> Result<(String, Option<Usage>), SummarizeFailure> {
    let abort: AbortSignal = env.abort.clone();
    let mut rx = env
        .provider
        .stream(env.model, context, options, env.abort.clone())
        .await;
    let mut summary_text = String::new();
    let mut usage: Option<Usage> = None;
    let mut failure: Option<SummarizeFailure> = None;
    let mut stop_reason: Option<StopReason> = None;
    // 流必须走到终态事件才算成功：干净 EOF 会把半截摘要当成功返回（曾经的缺口）
    let mut saw_done = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { delta, .. } => summary_text.push_str(&delta),
            StreamEvent::Done {
                reason,
                message,
                usage: done_usage,
            } => {
                usage = Some(done_usage);
                stop_reason = Some(reason);
                // 以最终消息为准（Thinking 等非文本块被排除）
                let text = message_text(&message);
                if !text.trim().is_empty() {
                    summary_text = text;
                }
                saw_done = true;
            }
            StreamEvent::Error { message, retry, .. } => {
                // 摘要调用不参与协议协商（它不经过 agent loop）：端点说协议不支持
                // 时原样当终态报错；本轮里已经协商成功的协议会由 provider 的
                // 进程内记忆在 auto 模式下覆盖到这次请求上；manual 只用声明协议。
                failure = Some(match retry {
                    Some(hint) => SummarizeFailure::Retryable {
                        message,
                        hint: Some(hint),
                    },
                    None => SummarizeFailure::Fatal(format!("{label}调用失败：{message}")),
                });
                break;
            }
            _ => {}
        }
        if abort.is_aborted() {
            return Err(SummarizeFailure::Fatal(format!("{label}调用已中止")));
        }
    }
    if abort.is_aborted() {
        return Err(SummarizeFailure::Fatal(format!("{label}调用已中止")));
    }
    if let Some(failure) = failure {
        return Err(failure);
    }
    if !saw_done {
        return Err(SummarizeFailure::Retryable {
            message: "流提前结束：未收到结束帧".into(),
            hint: Some(pipi_protocol::RetryHint::plain()),
        });
    }
    if stop_reason == Some(StopReason::Length) {
        return Err(SummarizeFailure::Truncated {
            text: summary_text,
            usage,
        });
    }
    if summary_text.trim().is_empty() {
        return Err(SummarizeFailure::Fatal(format!(
            "{label}调用失败：模型未返回{label}内容（结束原因：{:?}，输出 {} 个 token）",
            stop_reason.unwrap_or_default(),
            usage.as_ref().map_or(0, |u| u.output),
        )));
    }
    Ok((summary_text, usage))
}

#[cfg(test)]
mod tests {
    use crate::compaction::llm::{build_summary_prompt, find_keep_start, summarize};
    use crate::compaction::{LlmSummarize, Preparation, StrategyEnv};
    use crate::test_support::{assistant_text, tool_result};
    use pipi_protocol::provider::{EventStream, Provider};
    use pipi_protocol::{
        AbortSignal, Api, ContentBlock, Context, Message, Model, StopReason, StreamEvent,
        StreamOptions, Usage,
    };
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
                error_kind: None,
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
            reasoning_effort: None,
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

    /// 按脚本逐次回放的摘要 provider：用来验证「截断 → 压短重试」的第二次请求。
    struct ScriptedSummaryProvider {
        replies: Mutex<std::collections::VecDeque<(StopReason, Vec<ContentBlock>, Usage)>>,
        requested_max_tokens: Mutex<Vec<u32>>,
        prompts: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Provider for ScriptedSummaryProvider {
        async fn stream(
            &self,
            _model: &Model,
            context: &Context,
            options: &StreamOptions,
            _abort: AbortSignal,
        ) -> EventStream {
            self.requested_max_tokens
                .lock()
                .unwrap()
                .push(options.max_tokens.unwrap());
            let prompt = context
                .messages
                .iter()
                .map(|message| match message {
                    Message::User { content, .. } => content.text(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            self.prompts.lock().unwrap().push(prompt);
            let (reason, content, usage) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted summary reply");
            let message = Message::Assistant {
                content,
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage,
                stop_reason: reason,
                error_message: None,
                error_kind: None,
                timestamp: 0,
                duration_ms: None,
            };
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            tx.send(StreamEvent::Done {
                reason,
                usage,
                message: Box::new(message),
            })
            .await
            .unwrap();
            rx
        }
    }

    async fn scripted_summary(
        replies: Vec<(StopReason, &str, u64)>,
        summary_max: u32,
    ) -> (
        Result<(String, Option<Usage>), String>,
        Vec<u32>,
        Vec<String>,
    ) {
        let replies = replies
            .into_iter()
            .map(|(reason, text, output_tokens)| {
                let content = if text.is_empty() {
                    vec![ContentBlock::Thinking {
                        thinking: "reasoning only".into(),
                        thinking_signature: None,
                    }]
                } else {
                    vec![ContentBlock::Text { text: text.into() }]
                };
                (
                    reason,
                    content,
                    Usage {
                        output: output_tokens,
                        ..Default::default()
                    },
                )
            })
            .collect();
        let provider = ScriptedSummaryProvider {
            replies: Mutex::new(replies),
            requested_max_tokens: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
        };
        let model = Model {
            id: "test".into(),
            name: String::new(),
            api: Api::OpenAICompletions,
            base_url: String::new(),
            max_tokens: 384_000,
            context_window: 1_000_000,
            reasoning_effort: None,
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
        let requested = provider.requested_max_tokens.lock().unwrap().clone();
        let prompts = provider.prompts.lock().unwrap().clone();
        (result, requested, prompts)
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

    /// 第一次撞输出上限：把截断摘要喂回去压短，第二次成功。
    #[tokio::test]
    async fn truncated_summary_is_condensed_and_retried_once() {
        let (result, requested, prompts) = scripted_summary(
            vec![
                (StopReason::Length, "partial summary", 8_192),
                (StopReason::Stop, "condensed summary", 500),
            ],
            8_192,
        )
        .await;
        assert_eq!(result.unwrap().0, "condensed summary");
        // 第一次用原上限，第二次用一半（4,096）并要求压短。
        assert_eq!(requested, vec![8_192, 4_096]);
        assert_eq!(prompts.len(), 2);
        assert!(prompts[1].contains("partial summary"), "{}", prompts[1]);
        assert!(prompts[1].contains("truncated summary"), "{}", prompts[1]);
    }

    /// 截断文本为空（例如 reasoning 吃光输出预算）：不压短，直接 fail-closed。
    #[tokio::test]
    async fn empty_truncated_summary_is_not_condensed() {
        let (result, requested, _) =
            scripted_summary(vec![(StopReason::Length, "", 4_096)], 4_096).await;
        assert!(result.unwrap_err().contains("未保存可能截断的摘要"));
        assert_eq!(requested.len(), 1, "空截断不应再发第二次请求");
    }

    /// 压短重试只做一次：第二次仍被截断就放弃，不无限重试。
    #[tokio::test]
    async fn truncated_summary_retries_only_once() {
        let (result, requested, _) = scripted_summary(
            vec![
                (StopReason::Length, "partial summary", 4_096),
                (StopReason::Length, "still partial", 4_096),
            ],
            4_096,
        )
        .await;
        assert!(result.unwrap_err().contains("未保存可能截断的摘要"));
        assert_eq!(requested.len(), 2);
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
            reasoning_effort: None,
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
        // 摘要请求用的是「压缩前上下文 × 目标百分比」，不是 budget.summary_max。
        assert_eq!(
            *provider
                .requested_max_tokens
                .lock()
                .unwrap()
                .last()
                .unwrap(),
            target.budget.summary_max_for(target.tokens())
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
