//! LLM provider 层 —— 基于 [rig](https://crates.io/crates/rig-core) 的适配层。
//!
//! 采纳 opencode 的架构决策（它用 Vercel AI SDK）：provider 的 HTTP/SSE/协议
//! 演进交给第三方库维护，Pipi 只做两件事：
//! 1. 把 pi 风格的消息/工具模型映射到 rig 的请求模型
//! 2. 把 rig 的流式事件映射回 pi 风格的 [`StreamEvent`]（agent_loop 无感知）
//!
//! 错误不在 HTTP 层抛出，而是以 `StreamEvent::Error` 进入事件流 —— 与 pi 的
//! StreamFn 契约一致。

use std::time::Instant;

use async_trait::async_trait;
use futures_util::StreamExt;
use rig::client::CompletionClient;
use rig::completion::{CompletionRequestBuilder, FinishReason, Usage as RigUsage};
use rig::message::{
    AssistantContent, Message as RigMessage, ReasoningContent,
    ToolResultContent as RigToolResultContent,
};
use rig::streaming::StreamedAssistantContent;
use serde_json::{json, Value};
use tokio::sync::mpsc::{self, Sender};

use crate::types::{
    AbortSignal, Api, ContentBlock, Context, Message, Model, StopReason, StreamEvent,
    StreamOptions, ToolResultContent, Usage,
};

pub type EventStream = mpsc::Receiver<StreamEvent>;

#[async_trait]
pub trait Provider: Send + Sync {
    /// 发起流式请求并立即返回事件接收端。HTTP/协议错误以 `StreamEvent::Error`
    /// 终止；正常结束以 `StreamEvent::Done` 终止。channel 关闭（None）表示
    /// 请求被中止或异常断开。
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        abort: AbortSignal,
    ) -> EventStream;
}

pub fn provider_for(api: Api) -> std::sync::Arc<dyn Provider> {
    std::sync::Arc::new(RigProvider::new(api))
}

/// rig 适配器。按 [`Api`] 分派到 rig 的对应 provider client。
pub struct RigProvider {
    api: Api,
}

impl RigProvider {
    pub fn new(api: Api) -> Self {
        RigProvider { api }
    }
}

fn send_error(tx: &Sender<StreamEvent>, message: String) {
    let _ = tx.try_send(StreamEvent::Error { message });
}

// ---------------------------------------------------------------------------
// 请求映射：Pipi 模型 → rig
// ---------------------------------------------------------------------------

/// 把会话历史映射为 rig 消息。system prompt 不进 messages（走 builder 的
/// preamble）。我们的 ToolResult 对应 rig 的 user 角色 ToolResult 块。
pub fn to_rig_messages(context: &Context) -> Vec<RigMessage> {
    let mut out = Vec::new();
    for msg in &context.messages {
        match msg {
            Message::User { content, .. } => {
                out.push(RigMessage::User {
                    content: vec![rig::message::UserContent::Text(rig::message::Text::new(
                        content.clone(),
                    ))],
                });
            }
            Message::Assistant { content, .. } => {
                let blocks: Vec<AssistantContent> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(AssistantContent::Text(
                            rig::message::Text::new(text.clone()),
                        )),
                        ContentBlock::Thinking {
                            thinking,
                            thinking_signature,
                        } => Some(AssistantContent::Reasoning(rig::message::Reasoning {
                            id: None,
                            content: vec![ReasoningContent::Text {
                                text: thinking.clone(),
                                signature: thinking_signature.clone(),
                            }],
                        })),
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            let _ = content;
                            Some(AssistantContent::ToolCall(rig::message::ToolCall {
                                id: make_call_id(id),
                                function: rig::message::ToolFunction {
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                },
                                provider: None,
                                signature: None,
                                additional_params: None,
                            }))
                        }
                    })
                    .collect();
                if !blocks.is_empty() {
                    out.push(RigMessage::Assistant { id: None, content: blocks });
                }
            }
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                ..
            } => {
                let text = content
                    .iter()
                    .map(|c| match c {
                        ToolResultContent::Text { text } => text.clone(),
                        ToolResultContent::Image { .. } => "[image]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if *is_error {
                    format!("[error] {text}")
                } else {
                    text
                };
                out.push(RigMessage::User {
                    content: vec![rig::message::UserContent::ToolResult(
                        rig::message::ToolResult {
                            call: make_call_id(tool_call_id),
                            provider: None,
                            name: tool_name.clone(),
                            content: vec![RigToolResultContent::Text(rig::message::Text::new(
                                text,
                            ))],
                        },
                    )],
                });
            }
        }
    }
    out
}

fn make_call_id(id: &str) -> rig::message::ToolCallId {
    rig::message::ToolCallId::new(id)
        .unwrap_or_else(|| rig::message::ToolCallId::new(crate::session::new_id()).unwrap())
}

/// 工具定义同构映射。
pub fn to_rig_tools(context: &Context) -> Vec<rig::completion::ToolDefinition> {
    context
        .tools
        .iter()
        .map(|t| rig::completion::ToolDefinition {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 响应映射：rig 流 → Pipi StreamEvent
// ---------------------------------------------------------------------------

fn from_rig_usage(u: &RigUsage) -> Usage {
    let mut usage = Usage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cached_input_tokens,
        cache_write: u.cache_creation_input_tokens,
        total_tokens: u.total_tokens,
    };
    if usage.total_tokens == 0 {
        usage.total_tokens = usage.total();
    }
    usage
}

fn map_finish_reason(reason: Option<&FinishReason>, has_tool_calls: bool) -> StopReason {
    match reason {
        Some(FinishReason::ToolCalls) => StopReason::ToolUse,
        Some(FinishReason::Length) => StopReason::Length,
        Some(FinishReason::Stop) => StopReason::Stop,
        Some(FinishReason::ContentFilter) | Some(FinishReason::Other(_)) => StopReason::Stop,
        None => {
            // provider 没报结束原因：有工具调用就按 toolUse（pi 的推断行为）
            if has_tool_calls {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            }
        }
    }
}

/// 流式累积器：从事件流构建最终助手消息，保证 Done 里的 Message
/// 与我们发出的事件一致。
#[derive(Default)]
struct Accumulator {
    blocks: Vec<ContentBlock>,
}

impl Accumulator {
    fn text_delta(&mut self, delta: &str) {
        match self.blocks.last_mut() {
            Some(ContentBlock::Text { text }) => text.push_str(delta),
            _ => self.blocks.push(ContentBlock::Text { text: delta.to_string() }),
        }
    }

    fn thinking_delta(&mut self, delta: &str) {
        match self.blocks.last_mut() {
            Some(ContentBlock::Thinking { thinking, .. }) => thinking.push_str(delta),
            _ => self.blocks.push(ContentBlock::Thinking {
                thinking: delta.to_string(),
                thinking_signature: None,
            }),
        }
    }

    fn tool_call_delta(&mut self, delta: &str) {
        // 参数片段以 Value::String 暂存，tool_call_end 时替换为解析后的块
        if let Some(ContentBlock::ToolCall { arguments, .. }) = self.blocks.last_mut() {
            let raw = arguments.as_str().map(str::to_string).unwrap_or_default();
            *arguments = Value::String(format!("{raw}{delta}"));
        }
    }

    fn tool_call_start(&mut self, id: &str, name: &str) {
        self.blocks.push(ContentBlock::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({}),
        });
    }

    fn tool_call_end(&mut self, id: &str, name: &str, arguments: Value) {
        if let Some(pos) = self
            .blocks
            .iter()
            .rposition(|b| matches!(b, ContentBlock::ToolCall { .. }))
        {
            self.blocks[pos] = ContentBlock::ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments,
            };
        } else {
            self.blocks.push(ContentBlock::ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments,
            });
        }
    }

    fn finalize(&self, model_id: &str, usage: Usage, reason: StopReason, duration_ms: u64) -> Message {
        let content = self
            .blocks
            .iter()
            .map(|b| match b {
                ContentBlock::ToolCall { id, name, arguments } => {
                    let arguments = if let Some(raw) = arguments.as_str() {
                        serde_json::from_str(raw).unwrap_or(json!({}))
                    } else {
                        arguments.clone()
                    };
                    ContentBlock::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments,
                    }
                }
                other => other.clone(),
            })
            .collect();
        Message::Assistant {
            content,
            api: String::new(),
            provider: "rig".into(),
            model: model_id.to_string(),
            usage,
            stop_reason: reason,
            error_message: None,
            timestamp: crate::types::now_millis(),
            duration_ms: Some(duration_ms),
        }
    }
}

/// 流式生命周期：构造 client → builder → 迭代映射 → Done。
/// 两个协议的 builder 泛型不同，用宏在分派点统一流程。
macro_rules! run_with_model {
    ($tx:expr, $abort:expr, $model:expr, $context:expr, $options:expr, $completion_model:expr) => {{
        let tx = $tx;
        let model: &Model = &$model;
        let context: &Context = &$context;
        let options: &StreamOptions = &$options;
        let started = Instant::now();

        // rig 语义：builder 的 prompt 是最后一条消息，messages 是它之前的历史
        let mut msgs = to_rig_messages(context);
        let Some(prompt) = msgs.pop() else {
            return Err("空消息历史".into());
        };
        let mut builder = CompletionRequestBuilder::new($completion_model, prompt)
            .messages(msgs)
            .temperature_opt(options.temperature.map(f64::from));
        if let Some(max_tokens) = options.max_tokens {
            builder = builder.max_tokens(max_tokens as u64);
        }
        if let Some(system) = &context.system_prompt {
            builder = builder.preamble(system.clone());
        }
        if !context.tools.is_empty() {
            builder = builder.tools(to_rig_tools(context));
        }

        let mut stream = builder
            .stream()
            .await
            .map_err(|e| format!("请求失败: {e}"))?;

        let mut acc = Accumulator::default();
        let mut finish: Option<FinishReason> = None;
        let mut usage = RigUsage::default();
        let mut saw_final = false;

        loop {
            let item = tokio::select! {
                _ = $abort.wait_aborted() => None,
                item = stream.next() => item,
            };
            let Some(item) = item else { break };
            match item.map_err(|e| format!("流错误: {e}"))? {
                StreamedAssistantContent::Text(t) => {
                    acc.text_delta(&t.text);
                    let _ = tx
                        .send(StreamEvent::TextDelta { content_index: 0, delta: t.text })
                        .await;
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    acc.thinking_delta(&reasoning);
                    let _ = tx
                        .send(StreamEvent::ThinkingDelta { content_index: 0, delta: reasoning })
                        .await;
                }
                StreamedAssistantContent::Reasoning { .. } => {
                    // 聚合块：delta 已覆盖，忽略
                }
                StreamedAssistantContent::ToolCallDelta { content, .. } => {
                    use rig::streaming::ToolCallDeltaContent as D;
                    let fragment = match &content {
                        D::Delta(s) => s.clone(),
                        D::Name(_) => String::new(),
                    };
                    if !fragment.is_empty() {
                        acc.tool_call_delta(&fragment);
                        let _ = tx
                            .send(StreamEvent::ToolCallDelta { content_index: 0, delta: fragment })
                            .await;
                    }
                }
                StreamedAssistantContent::ToolCall { tool_call, .. } => {
                    let id = tool_call.id.to_string();
                    let name = tool_call.function.name.clone();
                    let arguments = tool_call.function.arguments.clone();
                    acc.tool_call_start(&id, &name);
                    acc.tool_call_end(&id, &name, arguments.clone());
                    let _ = tx
                        .send(StreamEvent::ToolCallEnd {
                            content_index: 0,
                            call: ContentBlock::ToolCall { id, name, arguments },
                        })
                        .await;
                }
                StreamedAssistantContent::Final(f) => {
                    usage = f.usage;
                    finish = f.finish_reason.clone();
                    saw_final = true;
                }
                StreamedAssistantContent::Unknown(_) => {}
            }
        }

        if !saw_final || $abort.is_aborted() {
            // 流异常断开或已中止：不发 Done，loop 按 aborted 处理
            return Ok(());
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        let has_tools = acc
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { .. }));
        let reason = map_finish_reason(finish.as_ref(), has_tools);
        let usage_mapped = from_rig_usage(&usage);
        let message = acc.finalize(&model.id, usage_mapped, reason, duration_ms);
        let _ = tx
            .send(StreamEvent::Done {
                reason,
                usage: usage_mapped,
                message: Box::new(message),
            })
            .await;
        Ok(())
    }};
}

#[async_trait]
impl Provider for RigProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        abort: AbortSignal,
    ) -> EventStream {
        let (tx, rx) = mpsc::channel(256);
        let api = self.api;
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();

        tokio::spawn(async move {
            async fn stream_impl(
                api: Api,
                model: Model,
                context: Context,
                options: StreamOptions,
                abort: AbortSignal,
                tx: &Sender<StreamEvent>,
            ) -> Result<(), String> {
                match api {
                Api::AnthropicMessages => {
                    let key = options.api_key.clone().unwrap_or_default();
                    let mut cb = rig::providers::anthropic::Client::builder().api_key(key);
                    if !model.base_url.is_empty() {
                        cb = cb.base_url(model.base_url.clone());
                    }
                    let client = cb.build().map_err(|e| format!("Client 初始化失败: {e}"))?;
                    run_with_model!(tx, abort, model, context, options, client.completion_model(&model.id))
                }
                Api::OpenAICompletions => {
                    let key = options.api_key.clone().unwrap_or_default();
                    let mut cb = rig::providers::openai::Client::builder().api_key(key);
                    if !model.base_url.is_empty() {
                        cb = cb.base_url(model.base_url.clone());
                    }
                    let client = cb.build().map_err(|e| format!("Client 初始化失败: {e}"))?;
                    run_with_model!(tx, abort, model, context, options, client.completion_model(&model.id))
                }
                }
            }
            if let Err(e) = stream_impl(api, model, context, options, abort, &tx).await {
                send_error(&tx, e);
            }
        });
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tool;

    fn context() -> Context {
        Context {
            system_prompt: Some("be brief".into()),
            messages: vec![
                Message::user_text("hello"),
                Message::Assistant {
                    content: vec![ContentBlock::ToolCall {
                        id: "t1".into(),
                        name: "read".into(),
                        arguments: json!({"path": "a.md"}),
                    }],
                    api: String::new(),
                    provider: String::new(),
                    model: "m".into(),
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 0,
                    duration_ms: None,
                },
                Message::ToolResult {
                    tool_call_id: "t1".into(),
                    tool_name: "read".into(),
                    content: vec![ToolResultContent::Text { text: "out".into() }],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                },
            ],
            tools: vec![Tool {
                name: "read".into(),
                description: "d".into(),
                parameters: json!({"type": "object"}),
            }],
        }
    }

    #[test]
    fn maps_messages_to_rig() {
        let msgs = to_rig_messages(&context());
        // user → assistant(toolcall) → user(toolresult)
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0], RigMessage::User { .. }));
        assert!(matches!(msgs[1], RigMessage::Assistant { .. }));
        match &msgs[2] {
            RigMessage::User { content } => match &content[0] {
                rig::message::UserContent::ToolResult(tr) => {
                    assert_eq!(tr.name, "read");
                    assert_eq!(tr.call.to_string(), "t1");
                }
                other => panic!("expected tool result, got {other:?}"),
            },
            other => panic!("expected user, got {other:?}"),
        }
    }

    #[test]
    fn maps_tools_to_rig() {
        let tools = to_rig_tools(&context());
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read");
        assert_eq!(tools[0].parameters["type"], json!("object"));
    }

    #[test]
    fn maps_finish_reasons() {
        assert_eq!(map_finish_reason(Some(&FinishReason::ToolCalls), false), StopReason::ToolUse);
        assert_eq!(map_finish_reason(Some(&FinishReason::Length), false), StopReason::Length);
        assert_eq!(map_finish_reason(Some(&FinishReason::Stop), false), StopReason::Stop);
        assert_eq!(map_finish_reason(None, true), StopReason::ToolUse);
        assert_eq!(map_finish_reason(None, false), StopReason::Stop);
    }

    #[test]
    fn maps_usage_with_cache() {
        let u = RigUsage {
            input_tokens: 100,
            output_tokens: 20,
            total_tokens: 0,
            cached_input_tokens: 80,
            cache_creation_input_tokens: 5,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let mapped = from_rig_usage(&u);
        assert_eq!(mapped.cache_read, 80);
        assert_eq!(mapped.cache_write, 5);
        // total 未上报时自行汇总
        assert_eq!(mapped.total_tokens, 205);
    }

    #[test]
    fn accumulator_finalizes_tool_args_and_duration() {
        let mut acc = Accumulator::default();
        acc.text_delta("hi");
        acc.tool_call_start("t1", "bash");
        acc.tool_call_delta("{\"c");
        acc.tool_call_delta("md\":\"ls\"}");
        acc.tool_call_end("t1", "bash", json!({"cmd": "ls"}));
        let msg = acc.finalize("m", Usage::default(), StopReason::ToolUse, 1234);
        let Message::Assistant { content, duration_ms, .. } = msg else {
            panic!("expected assistant");
        };
        assert_eq!(duration_ms, Some(1234));
        assert_eq!(content.len(), 2);
        assert_eq!(
            content[1],
            ContentBlock::ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                arguments: json!({"cmd": "ls"}),
            }
        );
    }
}
