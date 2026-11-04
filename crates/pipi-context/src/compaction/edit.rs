//! Agent 主导的上下文编辑（替换式）：模型看完整历史，返回要清理/删除的清单。
//!
//! 与摘要的区别：
//! - 不生成摘要，保留历史结构；
//! - 输入是编号后的完整历史（所以只在小上下文里启用，见
//!   [`super::AGENT_EDIT_MAX_INPUT_TOKENS`]）；
//! - 输出是 JSON 索引清单，运行时按原下标应用并落 `context_edit` 条目。
//!
//! 模型只做选择，不写正文：清理统一换成 [`super::tool_output`] 的占位符，
//! 删除按整轮展开、剥 thinking 保留其余块 —— 角色与工具配对不变量仍由
//! [`super::validate`] / [`super::validate_result`] 统一把关。

use std::collections::{BTreeSet, HashSet};

use super::llm::{call_plain, find_keep_start};
use super::tool_output::cleared_replacement;
use super::{
    AGENT_EDIT_MAX_INPUT_TOKENS, AGENT_EDIT_MAX_OUTPUT_TOKENS, ClearedOp, EditOps, Plan,
    Preparation, StrategyEnv,
};
use pipi_protocol::{ContentBlock, Context, Message, ToolResultContent, Usage};

/// 编辑调用的系统提示：限定为编辑器角色，禁止把对话接下去。
const EDIT_SYSTEM_PROMPT: &str = "You are a context editor. \
Do NOT continue the conversation. Do NOT respond to any questions in the conversation. \
ONLY return the requested JSON object.";

/// 指令 + 编号前缀的粗略开销：100k 门槛按「估算输入 + 这份开销」算。
const PROMPT_OVERHEAD_TOKENS: u64 = 4_000;

/// 给一次编辑调用预留的输出/窗口空间（模型窗口检查用）。
const OUTPUT_RESERVE_TOKENS: u64 = 8_000;

pub struct LlmEdit;

impl LlmEdit {
    pub(super) const NAME: &'static str = "agent-edit";

    pub(super) async fn run(
        &self,
        prep: &Preparation<'_>,
        env: &StrategyEnv<'_>,
    ) -> Result<Plan, String> {
        let measured = prep.tokens();
        let estimated = prep.estimated_tokens();
        let input_tokens = measured.max(estimated);
        let full_input = input_tokens.saturating_add(PROMPT_OVERHEAD_TOKENS);
        if full_input > AGENT_EDIT_MAX_INPUT_TOKENS {
            return Err(format!(
                "Agent compact is limited to about {}k tokens of context (currently ~{}k); use Summarize for larger sessions",
                AGENT_EDIT_MAX_INPUT_TOKENS / 1_000,
                full_input / 1_000
            ));
        }
        if prep.budget.context_window > 0
            && full_input.saturating_add(OUTPUT_RESERVE_TOKENS) > prep.budget.context_window
        {
            return Err(format!(
                "Agent compact needs the whole history in one request; the model window is too small ({} tokens)",
                prep.budget.context_window
            ));
        }

        let keep_tokens = prep.budget.keep_budget_for(measured);
        let Some(protected_start) = find_keep_start(prep.messages, keep_tokens) else {
            return Err("History is already within the keep budget; nothing to compact".into());
        };
        let target_tokens = match prep.budget.target_percent {
            Some(percent) => measured.saturating_mul(percent as u64) / 100,
            None => keep_tokens,
        };

        let (reply, usage) = request_plan(
            prep.messages,
            env,
            protected_start,
            target_tokens,
            keep_tokens,
            estimated,
        )
        .await?;
        let (rewrites, ops) = plan_edits(prep.messages, protected_start, reply)?;
        if ops.removed.is_empty() && ops.cleared.is_empty() && ops.thinking_removed.is_empty() {
            return Err("The model did not select any editable content".into());
        }
        Ok(Plan {
            rewrites,
            removals: ops.removed.clone(),
            usage,
            edit: Some(ops),
            ..Default::default()
        })
    }
}

/// 模型的 JSON 回复。蛇形与驼峰都接受，减少一次格式失误就整次失败的概率。
#[derive(Debug, Default)]
struct EditReply {
    clear_tool_results: Vec<usize>,
    drop_thinking: Vec<usize>,
    drop_turns: Vec<usize>,
    note: String,
}

async fn request_plan(
    messages: &[Message],
    env: &StrategyEnv<'_>,
    protected_start: usize,
    target_tokens: u64,
    keep_tokens: u64,
    estimated: u64,
) -> Result<(EditReply, Option<Usage>), String> {
    let context = Context {
        system_prompt: Some(EDIT_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user_text(build_edit_prompt(
            messages,
            protected_start,
            target_tokens,
            keep_tokens,
            estimated,
        ))],
        tools: vec![],
    };
    let mut options = env.options.clone();
    let model_max = if env.model.max_tokens == 0 {
        u32::MAX
    } else {
        env.model.max_tokens
    };
    options.max_tokens = Some(model_max.min(AGENT_EDIT_MAX_OUTPUT_TOKENS));
    let (text, usage) = call_plain(env, &context, &options, "编辑").await?;
    let mut reply = parse_reply(&text)?;
    reply.note = reply.note.trim().to_string();
    Ok((reply, usage))
}

/// 编号后的完整历史 + 编辑指令。模型只看到文本；索引就是消息下标。
fn build_edit_prompt(
    messages: &[Message],
    protected_start: usize,
    target_tokens: u64,
    keep_tokens: u64,
    estimated: u64,
) -> String {
    let transcript = render_transcript(messages);
    let tail = if protected_start < messages.len() {
        format!("Messages #{protected_start} onward are the recent tail and must not be edited.")
    } else {
        "The whole history is outside the protected tail, but the first user message and the last assistant message are still off-limits.".to_string()
    };
    format!(
        "Reduce the context usage of the conversation below by selecting edits.\n\n\
Available operations (all fields optional):\n\
- \"clear_tool_results\": tool result numbers whose content is obsolete; each becomes a short placeholder.\n\
- \"drop_thinking\": assistant message numbers whose thinking blocks are no longer needed.\n\
- \"drop_turns\": numbers inside dead-end turns to remove entirely (the whole turn, including its tool calls and results).\n\n\
Rules:\n\
- Never edit the first user message, any <compaction summary>, the last assistant message, or the recent tail.\n\
- Never break tool_call/tool_result pairs. Prefer clearing a result over dropping a turn.\n\
- Preserve task instructions, decisions, file paths, error messages still being debugged, and the current state.\n\
- {tail}\n\
- Target: about {target_tokens} tokens after the edit; keep the last {keep_tokens} tokens verbatim. Current estimate: {estimated} tokens.\n\n\
Return ONLY a JSON object, no prose:\n\
{{\"clear_tool_results\":[],\"drop_thinking\":[],\"drop_turns\":[],\"note\":\"one short sentence\"}}\n\n\
<conversation>\n\n{transcript}\n</conversation>"
    )
}

/// 每条消息带 `[#i]` 下标；tool_result 的正文原样给模型，thinking 也原样给
/// （否则模型无法判断哪些思考可以丢）。
fn render_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for (index, message) in messages.iter().enumerate() {
        match message {
            Message::User { content, .. } => {
                out.push_str(&format!("[#{index}] user:\n{}\n\n", content.text()));
            }
            Message::Assistant { content, .. } => {
                for block in content {
                    match block {
                        ContentBlock::Text { text } => {
                            out.push_str(&format!("[#{index}] assistant:\n{text}\n\n"));
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            out.push_str(&format!(
                                "[#{index}] assistant thinking:\n{thinking}\n\n"
                            ));
                        }
                        ContentBlock::ToolCall {
                            name, arguments, ..
                        } => {
                            out.push_str(&format!(
                                "[#{index}] assistant tool_call ({name}): {}\n\n",
                                serde_json::to_string(arguments).unwrap_or_default()
                            ));
                        }
                    }
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
                    .map(|part| match part {
                        ToolResultContent::Text { text } => text.clone(),
                        ToolResultContent::Image { .. } => "[image]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let tag = if *is_error { "error" } else { "ok" };
                out.push_str(&format!(
                    "[#{index}] tool result ({tool_name}, {tag}):\n{body}\n\n"
                ));
            }
        }
    }
    out
}

fn parse_reply(text: &str) -> Result<EditReply, String> {
    let body = text.trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .unwrap_or(body);
    let body = body.strip_suffix("```").unwrap_or(body);
    let start = body
        .find('{')
        .ok_or_else(|| "编辑调用未返回 JSON 对象".to_string())?;
    let end = body
        .rfind('}')
        .ok_or_else(|| "编辑调用未返回完整的 JSON 对象".to_string())?;
    let value: serde_json::Value = serde_json::from_str(&body[start..=end])
        .map_err(|error| format!("无法解析编辑计划：{error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "编辑计划必须是 JSON 对象".to_string())?;
    let indices = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| object.get(*key))
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .map(|index| index as usize)
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(EditReply {
        clear_tool_results: indices(&["clear_tool_results", "clearToolResults"]),
        drop_thinking: indices(&["drop_thinking", "dropThinking"]),
        drop_turns: indices(&["drop_turns", "dropTurns"]),
        note: object
            .get("note")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// 把模型的选择转换成「原下标 → 替换消息」+ 审计信息。
///
/// 保护规则在这里统一执行：被保护的选择直接跳过，不报错；模型偶尔多选一条
/// 不该动的消息，不值得让整次压缩失败。
fn plan_edits(
    messages: &[Message],
    protected_start: usize,
    reply: EditReply,
) -> Result<(Vec<(usize, Message)>, EditOps), String> {
    let protected = protected_indices(messages, protected_start);
    let mut removals: BTreeSet<usize> = BTreeSet::new();
    let mut rewrites: Vec<(usize, Message)> = Vec::new();
    let mut cleared: Vec<ClearedOp> = Vec::new();
    let mut thinking_removed: Vec<usize> = Vec::new();
    let mut clear_seen = HashSet::new();
    let mut thinking_seen = HashSet::new();

    for index in reply.drop_turns {
        let Some((start, end)) = turn_bounds(messages, index) else {
            continue;
        };
        // 整轮要么全删，要么不动：跨进保护尾部的轮次整体跳过。
        if start < protected_start && end > protected_start {
            continue;
        }
        if (start..end).any(|i| protected.contains(&i)) {
            continue;
        }
        removals.extend(start..end);
    }

    for index in reply.clear_tool_results {
        if index >= messages.len()
            || protected.contains(&index)
            || removals.contains(&index)
            || !clear_seen.insert(index)
        {
            continue;
        }
        let Some((replacement, text)) = cleared_replacement(&messages[index]) else {
            continue;
        };
        rewrites.push((index, replacement));
        cleared.push(ClearedOp { index, text });
    }

    for index in reply.drop_thinking {
        if index >= messages.len()
            || protected.contains(&index)
            || removals.contains(&index)
            || !thinking_seen.insert(index)
        {
            continue;
        }
        let Some(Message::Assistant {
            content,
            api,
            provider,
            model,
            usage,
            stop_reason,
            error_message,
            error_kind,
            timestamp,
            duration_ms,
        }) = messages.get(index)
        else {
            continue;
        };
        if !content
            .iter()
            .any(|block| matches!(block, ContentBlock::Thinking { .. }))
        {
            continue;
        }
        let without: Vec<ContentBlock> = content
            .iter()
            .filter(|block| !matches!(block, ContentBlock::Thinking { .. }))
            .cloned()
            .collect();
        if without.is_empty() {
            // 只有 thinking 的消息：整条删掉（没有 tool call，配对不受影响）。
            removals.insert(index);
            continue;
        }
        rewrites.push((
            index,
            Message::Assistant {
                content: without,
                api: api.clone(),
                provider: provider.clone(),
                model: model.clone(),
                usage: *usage,
                stop_reason: *stop_reason,
                error_message: error_message.clone(),
                error_kind: *error_kind,
                timestamp: *timestamp,
                duration_ms: *duration_ms,
            },
        ));
        thinking_removed.push(index);
    }

    // 删除优先：同一条消息不会既删又改（validate 也会拒绝）。
    rewrites.retain(|(index, _)| !removals.contains(index));
    cleared.retain(|op| !removals.contains(&op.index));
    thinking_removed.retain(|index| !removals.contains(index));
    rewrites.sort_by_key(|(index, _)| *index);
    cleared.sort_by_key(|op| op.index);
    thinking_removed.sort_unstable();

    let note = normalize_note(
        &reply.note,
        removals.len(),
        cleared.len(),
        thinking_removed.len(),
    );
    Ok((
        rewrites,
        EditOps {
            cleared,
            removed: removals.into_iter().collect(),
            thinking_removed,
            note,
        },
    ))
}

/// 不许动的下标：保护尾部、首条 user、所有摘要、最后一条 assistant。
fn protected_indices(messages: &[Message], protected_start: usize) -> HashSet<usize> {
    let mut protected: HashSet<usize> = (protected_start..messages.len()).collect();
    if let Some(first_user) = messages.iter().position(|message| {
        message.role() == "user" && pipi_protocol::session::summary_text(message).is_none()
    }) {
        protected.insert(first_user);
    }
    for (index, message) in messages.iter().enumerate() {
        if pipi_protocol::session::summary_text(message).is_some() {
            protected.insert(index);
        }
    }
    if let Some(last_assistant) = messages
        .iter()
        .rposition(|message| message.role() == "assistant")
    {
        protected.insert(last_assistant);
    }
    protected
}

/// 一轮 = 一条 user 消息到下一条 user 消息之前。`None` = 找不到轮首。
fn turn_bounds(messages: &[Message], index: usize) -> Option<(usize, usize)> {
    if index >= messages.len() {
        return None;
    }
    let start = (0..=index).rev().find(|&i| messages[i].role() == "user")?;
    let end = (start + 1..messages.len())
        .find(|&i| messages[i].role() == "user")
        .unwrap_or(messages.len());
    Some((start, end))
}

fn normalize_note(note: &str, removed: usize, cleared: usize, thinking: usize) -> String {
    let text = if note.trim().is_empty() {
        format!(
            "Removed {removed} message(s), cleared {cleared} tool output(s), dropped {thinking} thinking block(s)"
        )
    } else {
        note.trim().to_string()
    };
    text.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assistant_text, tool_result};
    use pipi_protocol::StopReason;

    fn user(text: &str) -> Message {
        Message::user_text(text)
    }

    fn assistant_with_thinking(text: &str) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::Thinking {
                thinking: text.into(),
                thinking_signature: None,
            }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    fn tool_call(id: &str) -> Message {
        Message::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "reason".into(),
                    thinking_signature: Some("sig".into()),
                },
                ContentBlock::ToolCall {
                    id: id.into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({ "command": "cargo build" }),
                },
            ],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    fn history() -> Vec<Message> {
        vec![
            user("task"),                                // 0 protected (first user)
            assistant_text("working"),                   // 1
            tool_call("t1"),                             // 2
            tool_result("t1", "bash", &"x".repeat(800)), // 3 clearable
            assistant_text("done with step"),            // 4
            user("continue"),                            // 5 protected tail (last turn)
            assistant_text("latest"),                    // 6 protected (last assistant)
        ]
    }

    fn reply(json: &str) -> EditReply {
        parse_reply(json).unwrap()
    }

    #[test]
    fn parses_fenced_json_and_rejects_garbage() {
        let plan = reply("```json\n{\"clear_tool_results\":[3],\"note\":\"ok\"}\n```");
        assert_eq!(plan.clear_tool_results, vec![3]);
        assert_eq!(plan.note, "ok");
        assert!(parse_reply("no json here").is_err());
    }

    #[test]
    fn clears_only_old_tool_results() {
        let messages = history();
        // protected_start = 5（最后一轮原文）
        let (rewrites, ops) = plan_edits(
            &messages,
            5,
            reply("{\"clear_tool_results\":[3,6],\"drop_thinking\":[3]}"),
        )
        .unwrap();
        assert_eq!(ops.cleared.len(), 1);
        assert_eq!(ops.cleared[0].index, 3);
        assert_eq!(rewrites.len(), 1);
        assert_eq!(ops.removed.len(), 0);
        let Message::ToolResult { content, .. } = &rewrites[0].1 else {
            panic!("expected tool result");
        };
        let ToolResultContent::Text { text } = &content[0] else {
            panic!("expected text");
        };
        assert!(text.starts_with(crate::compaction::tool_output::CLEARED_TOOL_RESULT_PREFIX));
        // #6 是最后一条 assistant，被保护；#3 不是 assistant，drop_thinking 无效。
        assert!(ops.thinking_removed.is_empty());
    }

    #[test]
    fn drops_a_whole_turn_and_keeps_pairing() {
        let messages = history();
        // #2/#3 属于第一轮（0..5），但一轮跨到 protected_start=5 之前就结束；
        // 删整轮会带走首条 user（#0 保护）→ 应整体跳过。
        let (_, ops) = plan_edits(&messages, 5, reply("{\"drop_turns\":[2]}")).unwrap();
        assert!(ops.removed.is_empty(), "首轮包含受保护的首条 user");

        // 构造一个可删的中间轮次。
        let mut messages = history();
        messages.insert(5, user("second task"));
        messages.insert(6, assistant_text("second answer"));
        messages.insert(7, user("third task"));
        messages.insert(8, assistant_text("third answer"));
        // protected_start 指向最后两轮
        let (_, ops) = plan_edits(&messages, 7, reply("{\"drop_turns\":[5]}")).unwrap();
        assert_eq!(ops.removed, vec![5, 6]);
        assert_eq!(messages[0].role(), "user");
    }

    #[test]
    fn drops_thinking_but_keeps_tool_calls() {
        let messages = history();
        let (rewrites, ops) = plan_edits(&messages, 5, reply("{\"drop_thinking\":[2]}")).unwrap();
        assert_eq!(ops.thinking_removed, vec![2]);
        let Message::Assistant { content, .. } = &rewrites[0].1 else {
            panic!("expected assistant");
        };
        assert!(
            content
                .iter()
                .all(|block| !matches!(block, ContentBlock::Thinking { .. }))
        );
        assert!(
            content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolCall { .. }))
        );
    }

    #[test]
    fn skips_protected_and_empty_plans() {
        let messages = history();
        let (_, ops) = plan_edits(
            &messages,
            5,
            reply("{\"clear_tool_results\":[0,5,6],\"drop_thinking\":[6]}"),
        )
        .unwrap();
        assert!(ops.cleared.is_empty());
        assert!(ops.thinking_removed.is_empty());
    }

    #[test]
    fn note_falls_back_to_counts() {
        let messages = history();
        let (_, ops) = plan_edits(&messages, 5, reply("{\"clear_tool_results\":[3]}")).unwrap();
        assert!(ops.note.contains("cleared 1"));
    }

    #[test]
    fn assistant_with_thinking_only_is_removed() {
        let mut messages = history();
        messages.insert(1, assistant_with_thinking("only thinking"));
        let (_, ops) = plan_edits(&messages, 5, reply("{\"drop_thinking\":[1]}")).unwrap();
        assert_eq!(ops.removed, vec![1]);
    }
}
