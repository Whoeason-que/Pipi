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
use http::{HeaderMap, HeaderName, HeaderValue};
use rig::client::CompletionClient;
use rig::completion::{CompletionError, CompletionRequestBuilder, FinishReason, Usage as RigUsage};
use rig::message::{
    AssistantContent, Message as RigMessage, ReasoningContent,
    ToolResultContent as RigToolResultContent,
};
use rig::streaming::StreamedAssistantContent;
use serde_json::{json, Value};
use tokio::sync::mpsc::{self, Sender};

use crate::retry::StreamError;
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

/// 需要「客户端自带会话标识」的供应商：键是 baseUrl 片段，值是要注入的会话头名。
///
/// 目前只有 OpenCode Go：它的文档要求第三方 coding agent 每个会话带稳定 ID
/// （<https://opencode.ai/docs/go/>，缺失直接 400 `MissingSessionID`），
/// 这样它才能做路由优化与 prompt 缓存。Hermes、Claude Code 等客户端都按此实现。
const SESSION_HEADER_PROVIDERS: &[(&str, &str)] = &[("opencode.ai/zen/go", "x-opencode-session")];

/// `StreamOptions.timeout_secs` 为 0 时的请求时限（秒）：无进展即中止。
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 300;

/// 我们自己的 User-Agent。供应商文档普遍要求客户端别用通用 SDK / HTTP 库的名字。
fn pipi_user_agent() -> String {
    format!("pipi/{}", env!("CARGO_PKG_VERSION"))
}

/// 按供应商组装额外请求头；没有特殊要求时返回 `None`（普通供应商一个头都不塞）。
fn extra_headers(base_url: &str, session_id: Option<&str>) -> Option<HeaderMap> {
    let (_, session_header) = SESSION_HEADER_PROVIDERS
        .iter()
        .find(|(pattern, _)| base_url.contains(pattern))?;
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&pipi_user_agent()) {
        headers.insert(HeaderName::from_static("user-agent"), value);
    }
    if let Some(id) = session_id.filter(|id| !id.is_empty()) {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(session_header.as_bytes()),
            HeaderValue::from_str(id),
        ) {
            headers.insert(name, value);
        }
    }
    Some(headers)
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

fn send_error(tx: &Sender<StreamEvent>, error: StreamError) {
    let _ = tx.try_send(StreamEvent::Error {
        message: error.message,
        retry: error.retry,
    });
}

/// 把 rig 错误转成带分类的 [`StreamError`]：分类表见 [`crate::retry`]。
fn classify_with(prefix: &str, error: CompletionError) -> StreamError {
    let (verdict, hint) = crate::retry::classify_rig_error(&error);
    let message = format!("{prefix}: {error}");
    match verdict {
        crate::retry::RetryVerdict::Retryable => StreamError::retryable_with(message, hint),
        crate::retry::RetryVerdict::Terminal => StreamError::terminal(message),
    }
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
                    .map(|b| match b {
                        ContentBlock::Text { text } => {
                            AssistantContent::Text(rig::message::Text::new(text.clone()))
                        }
                        ContentBlock::Thinking {
                            thinking,
                            thinking_signature,
                        } => AssistantContent::Reasoning(rig::message::Reasoning {
                            id: None,
                            content: vec![ReasoningContent::Text {
                                text: thinking.clone(),
                                signature: thinking_signature.clone(),
                            }],
                        }),
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            let _ = content;
                            AssistantContent::ToolCall(rig::message::ToolCall {
                                id: make_call_id(id),
                                function: rig::message::ToolFunction {
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                },
                                provider: None,
                                signature: None,
                                additional_params: None,
                            })
                        }
                    })
                    .collect();
                if !blocks.is_empty() {
                    out.push(RigMessage::Assistant {
                        id: None,
                        content: blocks,
                    });
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

/// rig 的用量 → Pipi（pi 口径）的 [`Usage`]。
///
/// rig 不统一口径：OpenAI 兼容路径把 `prompt_tokens` 原样塞进 `input_tokens`
/// （**已含**缓存命中部分），Anthropic 路径的 `input_tokens` 则不含缓存。若照抄，
/// `input` 与 `cache_read` 就重叠了 —— 上层按 `cache_read / (input + cache_read)`
/// 算命中率会永远得到 50%，上下文占用也翻倍。这里按协议归一化成 pi 的口径：
/// `input` 只计未命中部分，`input + cache_read + cache_write` 才是提示词总量。
fn from_rig_usage(u: &RigUsage, api: Api) -> Usage {
    let cache_read = u.cached_input_tokens;
    let cache_write = u.cache_creation_input_tokens;
    let input = if api.prompt_tokens_include_cache() {
        u.input_tokens
            .saturating_sub(cache_read)
            .saturating_sub(cache_write)
    } else {
        u.input_tokens
    };
    let mut usage = Usage {
        input,
        output: u.output_tokens,
        cache_read,
        cache_write,
        total_tokens: u.total_tokens,
    };
    // 端点没报总数时自行汇总（归一化之后才等于 wire 上的 prompt + completion）
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
            _ => self.blocks.push(ContentBlock::Text {
                text: delta.to_string(),
            }),
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

    fn finalize(
        &self,
        model_id: &str,
        usage: Usage,
        reason: StopReason,
        duration_ms: u64,
    ) -> Message {
        let content = self
            .blocks
            .iter()
            .map(|b| match b {
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
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
            return Err(StreamError::terminal("空消息历史"));
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

        // 请求级时限（无进展超时）：从发起请求到首个事件、以及相邻事件之间，
        // 超过时限没有任何数据即中止 —— 挂死的连接变成可见错误，而不是让会话
        // 无限期停在「运行中」。持续有增量的长响应不受影响。
        let idle_secs = if options.timeout_secs == 0 {
            DEFAULT_REQUEST_TIMEOUT_SECS
        } else {
            options.timeout_secs
        };
        let idle = std::time::Duration::from_secs(idle_secs);

        // 建连阶段同样与中止竞速：点停止不必等连接建立或超时
        let mut stream = tokio::select! {
            _ = $abort.wait_aborted() => return Ok(()),
            connected = tokio::time::timeout(idle, builder.stream()) => match connected {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => return Err(classify_with("请求失败", e)),
                Err(_) => {
                    return Err(StreamError::retryable_with(
                        format!("请求超时：{idle_secs} 秒内未开始响应（端点无响应或网络不可达）"),
                        Some(crate::retry::RetryHint::timeout()),
                    ))
                }
            },
        };

        let mut acc = Accumulator::default();
        let mut finish: Option<FinishReason> = None;
        let mut usage = RigUsage::default();
        let mut saw_final = false;
        let mut timed_out = false;

        loop {
            let item = tokio::select! {
                _ = $abort.wait_aborted() => None,
                item = stream.next() => item,
                // 每个事件到达都会重开计时：这是「无进展」超时，不是总时长超时
                _ = tokio::time::sleep(idle) => { timed_out = true; None }
            };
            let Some(item) = item else { break };
            match item.map_err(|e| classify_with("流错误", e))? {
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

        if timed_out {
            // 无进展超时：可重试，但每次尝试都要花掉整个 timeout 预算，
            // 所以带上 RetryHint::timeout()（重试层据此最多再试一次）。
            return Err(StreamError::retryable_with(
                format!("请求超时：{idle_secs} 秒内没有新的响应数据，已中止本轮请求"),
                Some(crate::retry::RetryHint::timeout()),
            ));
        }

        if $abort.is_aborted() {
            // 用户中止：静默收尾，agent_loop 按 aborted 处理（不可重试）
            return Ok(());
        }

        if !saw_final {
            // 流在终态事件之前结束（干净 EOF、代理掐断等）：过去这里静默结束，
            // 被上层误报成「已中止」；现在明确报成可重试的中断。
            return Err(StreamError::retryable(
                "流提前结束：未收到结束帧（连接被中断或代理截断）",
            ));
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        let has_tools = acc
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { .. }));
        let reason = map_finish_reason(finish.as_ref(), has_tools);
        let usage_mapped = from_rig_usage(&usage, model.api);
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
            ) -> Result<(), StreamError> {
                match api {
                    Api::AnthropicMessages => {
                        let key = options.api_key.clone().unwrap_or_default();
                        let mut cb = rig::providers::anthropic::Client::builder().api_key(key);
                        if !model.base_url.is_empty() {
                            cb = cb.base_url(model.base_url.clone());
                        }
                        if let Some(headers) =
                            extra_headers(&model.base_url, options.session_id.as_deref())
                        {
                            cb = cb.http_headers(headers);
                        }
                        let client = cb
                            .build()
                            .map_err(|e| StreamError::terminal(format!("Client 初始化失败: {e}")))?;
                        run_with_model!(
                            tx,
                            abort,
                            model,
                            context,
                            options,
                            client.completion_model(&model.id)
                        )
                    }
                    Api::OpenAICompletions => {
                        let key = options.api_key.clone().unwrap_or_default();
                        let mut cb = rig::providers::openai::Client::builder().api_key(key);
                        if !model.base_url.is_empty() {
                            cb = cb.base_url(model.base_url.clone());
                        }
                        if let Some(headers) =
                            extra_headers(&model.base_url, options.session_id.as_deref())
                        {
                            cb = cb.http_headers(headers);
                        }
                        let client = cb
                            .build()
                            .map_err(|e| StreamError::terminal(format!("Client 初始化失败: {e}")))?;
                        // 统一走 Chat Completions（`/chat/completions`）：rig 0.42 的 openai 客户端
                        // 默认是 Responses API（`/responses`），而「openai-completions」协议在中转商、
                        // 本地运行时那里就是 Chat Completions 的兼容层 —— 只有官方 OpenAI 才认
                        // /responses。协议名与实现必须一致，否则绝大多数兼容端点直接 404。
                        run_with_model!(
                            tx,
                            abort,
                            model,
                            context,
                            options,
                            client.completions_api().completion_model(&model.id)
                        )
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
        assert_eq!(
            map_finish_reason(Some(&FinishReason::ToolCalls), false),
            StopReason::ToolUse
        );
        assert_eq!(
            map_finish_reason(Some(&FinishReason::Length), false),
            StopReason::Length
        );
        assert_eq!(
            map_finish_reason(Some(&FinishReason::Stop), false),
            StopReason::Stop
        );
        assert_eq!(map_finish_reason(None, true), StopReason::ToolUse);
        assert_eq!(map_finish_reason(None, false), StopReason::Stop);
    }

    #[test]
    fn maps_usage_with_cache() {
        // Anthropic 口径：input_tokens 不含缓存，原样透传
        let u = RigUsage {
            input_tokens: 100,
            output_tokens: 20,
            total_tokens: 0,
            cached_input_tokens: 80,
            cache_creation_input_tokens: 5,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let mapped = from_rig_usage(&u, Api::AnthropicMessages);
        assert_eq!(mapped.input, 100);
        assert_eq!(mapped.cache_read, 80);
        assert_eq!(mapped.cache_write, 5);
        // total 未上报时自行汇总
        assert_eq!(mapped.total_tokens, 205);
    }

    #[test]
    fn openai_usage_subtracts_cached_tokens_from_input() {
        // OpenAI 兼容口径：prompt_tokens 已含 cached_tokens，必须扣掉，
        // 否则 input 与 cache_read 重叠 —— 命中率会恒为 50%
        let u = RigUsage {
            input_tokens: 1000,
            output_tokens: 50,
            total_tokens: 1050,
            cached_input_tokens: 900,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let mapped = from_rig_usage(&u, Api::OpenAICompletions);
        assert_eq!(mapped.input, 100);
        assert_eq!(mapped.cache_read, 900);
        // 提示词总量回到 wire 上的 prompt_tokens，总数不变
        assert_eq!(mapped.prompt_tokens(), 1000);
        assert_eq!(mapped.total_tokens, 1050);
        assert_eq!(mapped.total(), 1050);
        // 命中率：900 / 1000 = 90%
        let hit = mapped.cache_read as f64 / mapped.prompt_tokens() as f64;
        assert!((hit - 0.9).abs() < 1e-9, "hit={hit}");
    }

    #[test]
    fn openai_usage_without_total_stays_consistent() {
        // 端点不报 total_tokens：汇总值也要等于 prompt + completion（不能把命中算两遍）
        let u = RigUsage {
            input_tokens: 610_566,
            output_tokens: 165,
            total_tokens: 0,
            cached_input_tokens: 610_432,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let mapped = from_rig_usage(&u, Api::OpenAICompletions);
        assert_eq!(mapped.input, 134);
        assert_eq!(mapped.total_tokens, 610_731);
        assert_eq!(mapped.total_tokens, mapped.total());
        // 真实会话的命中率：610432 / 610566 ≈ 99.98%，而不是 50%
        let pct = mapped.cache_read as f64 / mapped.prompt_tokens() as f64 * 100.0;
        assert!((pct - 99.98).abs() < 0.01, "pct={pct}");
    }

    #[test]
    fn openai_usage_saturates_when_cache_exceeds_prompt() {
        // 端点口径混乱（cached > prompt）时不能下溢成天文数字
        let u = RigUsage {
            input_tokens: 10,
            output_tokens: 1,
            total_tokens: 11,
            cached_input_tokens: 500,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let mapped = from_rig_usage(&u, Api::OpenAICompletions);
        assert_eq!(mapped.input, 0);
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
        let Message::Assistant {
            content,
            duration_ms,
            ..
        } = msg
        else {
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

    #[test]
    fn extra_headers_only_for_providers_that_require_session_id() {
        // 普通供应商：一个头都不塞，避免给无关请求改变行为
        assert!(extra_headers("https://api.deepseek.com/v1", Some("s-1")).is_none());
        assert!(extra_headers("https://api.anthropic.com", None).is_none());
        assert!(extra_headers("", Some("s-1")).is_none());

        // OpenCode Go：UA + 会话头都要有
        let headers = extra_headers("https://opencode.ai/zen/go/v1", Some("1789230239133-abc"))
            .expect("opencode-go 应带额外请求头");
        assert_eq!(
            headers
                .get("x-opencode-session")
                .and_then(|v| v.to_str().ok()),
            Some("1789230239133-abc"),
        );
        let user_agent = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            user_agent.starts_with("pipi/"),
            "UA 应为 pipi/<版本>，实际 {user_agent}"
        );
    }

    #[test]
    fn extra_headers_without_session_id_still_identifies_client() {
        let headers =
            extra_headers("https://opencode.ai/zen/go/v1", None).expect("命中供应商时至少应带 UA");
        assert!(headers.get("x-opencode-session").is_none());
        assert!(headers.get("user-agent").is_some());

        // 空串视为没有会话 ID，不得塞空值头（HeaderValue 允许空串，但供应商侧无法路由）
        let headers = extra_headers("https://opencode.ai/zen/go/v1", Some("")).unwrap();
        assert!(headers.get("x-opencode-session").is_none());
    }
}
