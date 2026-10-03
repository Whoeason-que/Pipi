//! LLM provider 层 —— 基于 [rig](https://crates.io/crates/rig-core) 的适配层。
//!
//! 采纳 opencode 的架构决策（它用 Vercel AI SDK）：provider 的 HTTP/SSE/协议
//! 演进交给第三方库维护，Pipi 只做两件事：
//! 1. 把 pi 风格的消息/工具模型映射到 rig 的请求模型
//! 2. 把 rig 的流式事件映射回 pi 风格的 [`StreamEvent`]（agent_loop 无感知）
//!
//! 错误不在 HTTP 层抛出，而是以 `StreamEvent::Error` 进入事件流 —— 与 pi 的
//! StreamFn 契约一致。
//!
//! rig 0.43 换成了「wire + transport + fold」架构，与 0.42 的几处映射偏差：
//! - core 不再自带 HTTP transport：reqwest 由 `rig-reqwest` 提供，客户端 =
//!   配置（`AnthropicConfig` / `OpenAIConfig`）`.connect(transport)`；
//! - client 级的 `http_headers` 没了，额外请求头改走 transport 中间件
//!   （见 [`ExtraHeaders`]）；
//! - 流事件换成 [`rig::streaming::StreamEvent`] 的 part 生命周期
//!   （Start / Text / Reasoning / Arguments / End），用量与结束原因只在
//!   `Streamed::finish()` 的响应里；
//! - `Usage` 七个计数都是 `Option<u64>`，且各供应商统一成
//!   「`input_tokens` 含缓存读写」一套口径（见 [`from_rig_usage`]）。

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use async_trait::async_trait;
use futures_util::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue};
use rig::completion::{CompletionRequest, FinishReason, ToolDefinition, Usage as RigUsage};
use rig::http_client::{DynHttpClient, HttpMiddleware};
use rig::message::{
    AssistantContent, CallId, Message as RigMessage, ReasoningContent, ToolName,
    ToolResultContent as RigToolResultContent,
};
use rig::providers::anthropic::AnthropicConfig;
use rig::providers::openai::OpenAIConfig;
use rig::streaming::{Item as RigItem, StreamEvent as RigStreamEvent};
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, Sender};

mod classify;

use classify::{RetryVerdict, classify_rig_error};
use pipi_error::RetryHint;
use pipi_protocol::{
    AbortSignal, Api, ContentBlock, Context, Message, Model, StopReason, StreamEvent,
    StreamOptions, ToolResultContent, Usage,
};

pub type EventStream = mpsc::Receiver<StreamEvent>;

/// provider 内部错误：只携带要写入事件流的安全消息和重试事实。是否重发始终由
/// 调用方的 agent loop 决定，provider 本身不重试。
#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamError {
    message: String,
    retry: Option<RetryHint>,
}

impl StreamError {
    fn terminal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: None,
        }
    }

    fn retryable_with(message: impl Into<String>, hint: Option<RetryHint>) -> Self {
        Self {
            message: message.into(),
            retry: Some(hint.unwrap_or_else(RetryHint::plain)),
        }
    }
}

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

/// reqwest client owns the connection pool. Rig clients are request-scoped because
/// their API key, base URL, and extra headers vary; cloning this client keeps those
/// per-request settings separate while reusing idle HTTP connections.
fn shared_http_client() -> &'static reqwest::Client {
    static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

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
    if let Some(id) = session_id.filter(|id| !id.is_empty())
        && let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(session_header.as_bytes()),
            HeaderValue::from_str(id),
        )
    {
        headers.insert(name, value);
    }
    Some(headers)
}

/// 把 [`extra_headers`] 注入 transport：rig 0.43 移除了 client 级的
/// `http_headers`，额外请求头只能在这一层加。
struct ExtraHeaders(HeaderMap);

impl HttpMiddleware for ExtraHeaders {
    fn before_request_headers<'a>(
        &'a self,
        _method: &'a http::Method,
        _uri: &'a http::Uri,
        headers: &'a mut HeaderMap,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, rig::http_client::Result<()>> {
        Box::pin(async move {
            for (name, value) in &self.0 {
                headers.insert(name, value.clone());
            }
            Ok(())
        })
    }
}

/// rig 0.43 的 transport：core 不再自带 HTTP 栈，reqwest 由 `rig-reqwest` 提供。
/// 复用同一份 `reqwest::Client`，连接池因此仍跨请求共享。
fn rig_transport(extra: Option<HeaderMap>) -> DynHttpClient {
    let transport = DynHttpClient::new(rig::rig_reqwest::ReqwestClient::from(
        shared_http_client().clone(),
    ));
    match extra {
        Some(headers) => transport.with_middleware(ExtraHeaders(headers)),
        None => transport,
    }
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

/// 把 rig 错误转成带分类的 [`StreamError`]；分类表位于本 crate，provider
/// 只记录重试事实，不进行重发。
fn classify_with(prefix: &str, error: &rig::ProviderError) -> StreamError {
    let (verdict, hint) = classify_rig_error(error);
    let message = format!("{prefix}: {error}");
    match verdict {
        RetryVerdict::Retryable => StreamError::retryable_with(message, hint),
        RetryVerdict::Terminal => StreamError::terminal(message),
    }
}

// ---------------------------------------------------------------------------
// 请求映射：Pipi 模型 → rig
// ---------------------------------------------------------------------------

/// 思考块重放时的 issuer：rig 0.43 要求 reasoning 只能被「签发它的那家」读回，
/// 各 wire 用方言名做 issuer（anthropic / openai）。签发错家会被静默丢弃。
fn thinking_issuer(api: Api) -> &'static str {
    match api {
        Api::AnthropicMessages => "anthropic",
        Api::OpenAICompletions => "openai",
    }
}

fn tool_name(name: &str) -> ToolName {
    ToolName::new(name).unwrap_or_else(|_| ToolName::new("tool").expect("非空兜底名不应失败"))
}

/// 把会话历史映射为 rig 消息。system prompt 不进 messages（走请求的
/// preamble）。我们的 ToolResult 对应 rig 的 user 角色 ToolResult 块。
pub fn to_rig_messages(context: &Context, api: Api) -> Vec<RigMessage> {
    let issuer = thinking_issuer(api);
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
                        } => AssistantContent::Reasoning(
                            rig::message::Reasoning {
                                id: None,
                                content: vec![ReasoningContent::Text {
                                    text: thinking.clone(),
                                    signature: thinking_signature.clone(),
                                }],
                            }
                            .sealed(issuer),
                        ),
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            let _ = content;
                            AssistantContent::ToolCall(rig::message::ToolCall {
                                id: CallId::from_wire(id.clone()),
                                function: rig::message::ToolFunction {
                                    name: tool_name(name),
                                    arguments: arguments.clone(),
                                },
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
                tool_name: name,
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
                            call: CallId::from_wire(tool_call_id.clone()),
                            name: tool_name(name),
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

/// 工具定义同构映射。
pub fn to_rig_tools(context: &Context) -> Vec<ToolDefinition> {
    context
        .tools
        .iter()
        .map(|t| {
            ToolDefinition::new(
                tool_name(&t.name),
                t.description.clone(),
                t.parameters.clone(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 响应映射：rig 流 → Pipi StreamEvent
// ---------------------------------------------------------------------------

/// rig 的用量 → Pipi（pi 口径）的 [`Usage`]。
///
/// rig 0.43 起各家统一成一套口径：`input_tokens` **包含**缓存读与缓存写。
/// Pipi 沿用 pi 的口径：`input` 只计未命中部分，`input + cache_read +
/// cache_write` 才是提示词总量。若照抄，`input` 与 `cache_read` 就重叠了 ——
/// 上层按 `cache_read / (input + cache_read)` 算命中率会永远得到 50%，
/// 上下文占用也翻倍。这里统一扣掉缓存部分。
fn from_rig_usage(u: &RigUsage) -> Usage {
    let cache_read = u.cached_input_tokens.unwrap_or(0);
    let cache_write = u.cache_creation_input_tokens.unwrap_or(0);
    let prompt = u.input_tokens.unwrap_or(0);
    let output = u.output_tokens.unwrap_or(0);
    let mut usage = Usage {
        input: prompt
            .saturating_sub(cache_read)
            .saturating_sub(cache_write),
        output,
        cache_read,
        cache_write,
        total_tokens: u.total_tokens.unwrap_or(0),
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
///
/// rig 0.43 的事件带 part 下标（文本 / 思考 / 每个工具调用各占一个），
/// 按 part 归位可以避免并行工具调用的参数互相串台。
#[derive(Default)]
struct Accumulator {
    blocks: Vec<ContentBlock>,
    /// rig 的 part 下标 → `blocks` 下标
    parts: HashMap<usize, usize>,
}

impl Accumulator {
    /// 该 part 已有的块下标；类型不符或还没建块时按 `make` 新建一个。
    fn ensure_block(
        &mut self,
        part: usize,
        is_kind: impl Fn(&ContentBlock) -> bool,
        make: impl FnOnce() -> ContentBlock,
    ) -> usize {
        if let Some(&idx) = self.parts.get(&part)
            && is_kind(&self.blocks[idx])
        {
            return idx;
        }
        self.blocks.push(make());
        let idx = self.blocks.len() - 1;
        self.parts.insert(part, idx);
        idx
    }

    fn text_delta(&mut self, part: usize, delta: &str) {
        let idx = self.ensure_block(
            part,
            |b| matches!(b, ContentBlock::Text { .. }),
            || ContentBlock::Text {
                text: String::new(),
            },
        );
        if let ContentBlock::Text { text } = &mut self.blocks[idx] {
            text.push_str(delta);
        }
    }

    fn thinking_delta(&mut self, part: usize, delta: &str) {
        let idx = self.ensure_block(
            part,
            |b| matches!(b, ContentBlock::Thinking { .. }),
            || ContentBlock::Thinking {
                thinking: String::new(),
                thinking_signature: None,
            },
        );
        if let ContentBlock::Thinking { thinking, .. } = &mut self.blocks[idx] {
            thinking.push_str(delta);
        }
    }

    fn tool_call_delta(&mut self, part: usize, delta: &str) {
        // 参数片段以 Value::String 暂存，finalize 时替换为解析后的块
        let idx = self.ensure_block(
            part,
            |b| matches!(b, ContentBlock::ToolCall { .. }),
            || ContentBlock::ToolCall {
                id: String::new(),
                name: String::new(),
                arguments: Value::String(String::new()),
            },
        );
        if let ContentBlock::ToolCall { arguments, .. } = &mut self.blocks[idx] {
            let raw = arguments.as_str().map(str::to_string).unwrap_or_default();
            *arguments = Value::String(format!("{raw}{delta}"));
        }
    }

    fn tool_call_end(&mut self, part: usize, id: &str, name: &str, arguments: Value) {
        let idx = self.ensure_block(
            part,
            |b| matches!(b, ContentBlock::ToolCall { .. }),
            || ContentBlock::ToolCall {
                id: String::new(),
                name: String::new(),
                arguments: json!({}),
            },
        );
        self.blocks[idx] = ContentBlock::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        };
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
            timestamp: pipi_protocol::now_millis(),
            duration_ms: Some(duration_ms),
        }
    }
}

/// 流式生命周期：构造请求 → 迭代映射 → finish 取用量/结束原因 → Done。
/// 两个协议的 wire 类型不同（Messages / Chat），用宏在分派点统一流程。
macro_rules! run_with_model {
    ($tx:expr, $abort:expr, $model:expr, $context:expr, $options:expr, $rig_model:expr) => {{
        let tx = $tx;
        let model: &Model = &$model;
        let context: &Context = &$context;
        let options: &StreamOptions = &$options;
        let started = Instant::now();

        // rig 语义：请求的 prompt 是最后一条消息，messages 是它之前的历史
        let mut msgs = to_rig_messages(context, model.api);
        let Some(prompt) = msgs.pop() else {
            return Err(StreamError::terminal("空消息历史"));
        };
        let mut request = CompletionRequest::new(prompt)
            .messages(msgs)
            .temperature(options.temperature.map(f64::from));
        if let Some(max_tokens) = options.max_tokens {
            request = request.max_tokens(max_tokens as u64);
        }
        if let Some(system) = &context.system_prompt {
            request = request.preamble(system.clone());
        }
        if !context.tools.is_empty() {
            request = request.tools(to_rig_tools(context));
        }

        // rig 0.43：编码/校验失败在这里返回；传输失败是流里的最后一个事件项。
        let mut stream = $rig_model
            .stream(request)
            .map_err(|e| classify_with("请求失败", &e))?;

        // 请求级时限（无进展超时）：首个事件、以及相邻事件之间超过时限没有
        // 任何数据即中止 —— 挂死的连接变成可见错误，而不是让会话无限期停在
        // 「运行中」。持续有增量的长响应不受影响。
        let idle_secs = if options.timeout_secs == 0 {
            DEFAULT_REQUEST_TIMEOUT_SECS
        } else {
            options.timeout_secs
        };
        let idle = std::time::Duration::from_secs(idle_secs);

        let mut acc = Accumulator::default();
        let mut timed_out = false;

        loop {
            let item = tokio::select! {
                _ = $abort.wait_aborted() => None,
                item = stream.next() => item,
                // 每个事件到达都会重开计时：这是「无进展」超时，不是总时长超时
                _ = tokio::time::sleep(idle) => { timed_out = true; None }
            };
            let Some(item) = item else { break };
            let event = item.map_err(|e| classify_with("流错误", &e))?;
            match event {
                RigItem::Event(RigStreamEvent::Text { part, text }) => {
                    acc.text_delta(part.index(), &text);
                    let _ = tx
                        .send(StreamEvent::TextDelta {
                            content_index: 0,
                            delta: text,
                        })
                        .await;
                }
                RigItem::Event(RigStreamEvent::Reasoning { part, text }) => {
                    acc.thinking_delta(part.index(), &text);
                    let _ = tx
                        .send(StreamEvent::ThinkingDelta {
                            content_index: 0,
                            delta: text,
                        })
                        .await;
                }
                RigItem::Event(RigStreamEvent::Arguments { part, json }) => {
                    if !json.is_empty() {
                        acc.tool_call_delta(part.index(), &json);
                        let _ = tx
                            .send(StreamEvent::ToolCallDelta {
                                content_index: 0,
                                delta: json,
                            })
                            .await;
                    }
                }
                RigItem::Event(RigStreamEvent::End {
                    part,
                    content: AssistantContent::ToolCall(call),
                }) => {
                    let id = call.id.to_string();
                    let name = call.function.name.as_str().to_string();
                    let arguments = call.function.arguments.clone();
                    acc.tool_call_end(part.index(), &id, &name, arguments.clone());
                    let _ = tx
                        .send(StreamEvent::ToolCallEnd {
                            content_index: 0,
                            call: ContentBlock::ToolCall { id, name, arguments },
                        })
                        .await;
                }
                // 文本 / 思考 / 图片的 End 是聚合块：delta 已覆盖，忽略
                RigItem::Event(
                    RigStreamEvent::Start { .. }
                    | RigStreamEvent::End { .. },
                ) => {}
                // 未建模的供应商载荷：跳过（与 0.42 的 Unknown 处理一致）
                RigItem::Unknown(_) => {}
            }
        }

        if timed_out {
            // 无进展超时：可重试，但每次尝试都要花掉整个 timeout 预算，
            // 所以带上 RetryHint::timeout()（重试层据此最多再试一次）。
            return Err(StreamError::retryable_with(
                format!("请求超时：{idle_secs} 秒内没有新的响应数据，已中止本轮请求"),
                Some(RetryHint::timeout()),
            ));
        }

        if $abort.is_aborted() {
            // 用户中止：静默收尾，agent_loop 按 aborted 处理（不可重试）
            return Ok(());
        }

        // 流被截断（干净 EOF、代理掐断等）由 rig 报成 Truncated 的最后一项，
        // 上面已按可重试分类；能走到这里说明收到过结束帧。
        let response = stream
            .finish()
            .await
            .map_err(|e| classify_with("流错误", &e))?;

        let duration_ms = started.elapsed().as_millis() as u64;
        let has_tools = acc
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { .. }));
        let reason = map_finish_reason(response.finish_reason().as_ref(), has_tools);
        let usage_mapped = from_rig_usage(&response.usage);
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
                let key = options.api_key.clone().unwrap_or_default();
                let transport = rig_transport(extra_headers(
                    &model.base_url,
                    options.session_id.as_deref(),
                ));
                match api {
                    Api::AnthropicMessages => {
                        let mut config = AnthropicConfig::new(key);
                        if !model.base_url.is_empty() {
                            config = config.with_base_url(&model.base_url);
                        }
                        run_with_model!(
                            tx,
                            abort,
                            model,
                            context,
                            options,
                            config.connect(transport).completion(&model.id)
                        )
                    }
                    Api::OpenAICompletions => {
                        let mut config = OpenAIConfig::new(key);
                        if !model.base_url.is_empty() {
                            config = config.with_base_url(&model.base_url);
                        }
                        // 统一走 Chat Completions（`/chat/completions`）：
                        // 「openai-completions」协议在中转商、本地运行时那里就是
                        // Chat Completions 的兼容层 —— 只有官方 OpenAI 才认
                        // /responses。协议名与实现必须一致，否则绝大多数兼容
                        // 端点直接 404。
                        run_with_model!(
                            tx,
                            abort,
                            model,
                            context,
                            options,
                            config.connect(transport).chat(&model.id)
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
    use pipi_protocol::Tool;

    #[test]
    fn rig_calls_share_the_same_http_client_pool() {
        assert!(std::ptr::eq(shared_http_client(), shared_http_client()));
    }

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
        let msgs = to_rig_messages(&context(), Api::AnthropicMessages);
        // user → assistant(toolcall) → user(toolresult)
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0], RigMessage::User { .. }));
        assert!(matches!(msgs[1], RigMessage::Assistant { .. }));
        match &msgs[2] {
            RigMessage::User { content } => match &content[0] {
                rig::message::UserContent::ToolResult(tr) => {
                    assert_eq!(tr.name.as_str(), "read");
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
        assert_eq!(tools[0].name.as_str(), "read");
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
    fn usage_subtracts_cached_tokens_from_input() {
        // rig 0.43 的统一口径：input_tokens 已含缓存读写，必须扣掉，
        // 否则 input 与 cache_read 重叠 —— 命中率会恒为 50%
        let u = RigUsage {
            input_tokens: Some(1000),
            output_tokens: Some(50),
            total_tokens: Some(1050),
            cached_input_tokens: Some(900),
            cache_creation_input_tokens: Some(0),
            tool_use_prompt_tokens: Some(0),
            reasoning_tokens: Some(0),
        };
        let mapped = from_rig_usage(&u);
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
    fn usage_with_cache_write_is_also_subtracted() {
        let u = RigUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            total_tokens: None,
            cached_input_tokens: Some(80),
            cache_creation_input_tokens: Some(5),
            tool_use_prompt_tokens: None,
            reasoning_tokens: None,
        };
        let mapped = from_rig_usage(&u);
        assert_eq!(mapped.input, 15);
        assert_eq!(mapped.cache_read, 80);
        assert_eq!(mapped.cache_write, 5);
        // total 未上报时自行汇总
        assert_eq!(mapped.total_tokens, 120);
    }

    #[test]
    fn usage_without_total_stays_consistent() {
        // 端点不报 total_tokens：汇总值也要等于 prompt + completion（不能把命中算两遍）
        let u = RigUsage {
            input_tokens: Some(610_566),
            output_tokens: Some(165),
            total_tokens: None,
            cached_input_tokens: Some(610_432),
            cache_creation_input_tokens: Some(0),
            tool_use_prompt_tokens: None,
            reasoning_tokens: None,
        };
        let mapped = from_rig_usage(&u);
        assert_eq!(mapped.input, 134);
        assert_eq!(mapped.total_tokens, 610_731);
        assert_eq!(mapped.total_tokens, mapped.total());
        // 真实会话的命中率：610432 / 610566 ≈ 99.98%，而不是 50%
        let pct = mapped.cache_read as f64 / mapped.prompt_tokens() as f64 * 100.0;
        assert!((pct - 99.98).abs() < 0.01, "pct={pct}");
    }

    #[test]
    fn usage_saturates_when_cache_exceeds_prompt() {
        // 端点口径混乱（cached > prompt）时不能下溢成天文数字
        let u = RigUsage {
            input_tokens: Some(10),
            output_tokens: Some(1),
            total_tokens: Some(11),
            cached_input_tokens: Some(500),
            cache_creation_input_tokens: Some(0),
            tool_use_prompt_tokens: None,
            reasoning_tokens: None,
        };
        let mapped = from_rig_usage(&u);
        assert_eq!(mapped.input, 0);
    }

    #[test]
    fn usage_missing_counters_read_as_zero() {
        // 计数缺失（None）与上报 0 在 Pipi 口径里都记 0；总数仍能自洽
        let u = RigUsage::default();
        let mapped = from_rig_usage(&u);
        assert_eq!(mapped.input, 0);
        assert_eq!(mapped.total_tokens, 0);
    }

    #[test]
    fn accumulator_finalizes_tool_args_and_duration() {
        let mut acc = Accumulator::default();
        acc.text_delta(0, "hi");
        acc.tool_call_delta(1, "{\"c");
        acc.tool_call_delta(1, "md\":\"ls\"}");
        acc.tool_call_end(1, "t1", "bash", json!({"cmd": "ls"}));
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
    fn accumulator_keeps_parallel_tool_calls_apart() {
        // 两个工具调用的参数片段交错到达：按 part 归位，不能互相串台
        let mut acc = Accumulator::default();
        acc.tool_call_delta(0, "{\"a\":1}");
        acc.tool_call_delta(1, "{\"b\":2}");
        acc.tool_call_delta(0, "");
        acc.tool_call_end(0, "t1", "one", json!({"a": 1}));
        acc.tool_call_end(1, "t2", "two", json!({"b": 2}));
        let msg = acc.finalize("m", Usage::default(), StopReason::ToolUse, 1);
        let Message::Assistant { content, .. } = msg else {
            panic!("expected assistant");
        };
        assert_eq!(
            content,
            vec![
                ContentBlock::ToolCall {
                    id: "t1".into(),
                    name: "one".into(),
                    arguments: json!({"a": 1}),
                },
                ContentBlock::ToolCall {
                    id: "t2".into(),
                    name: "two".into(),
                    arguments: json!({"b": 2}),
                },
            ]
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

    #[test]
    fn thinking_blocks_replay_with_the_matching_issuer() {
        // reasoning 只能被签发它的那家读回：两套协议的 issuer 必须与自己一致
        assert_eq!(thinking_issuer(Api::AnthropicMessages), "anthropic");
        assert_eq!(thinking_issuer(Api::OpenAICompletions), "openai");
        let context = Context {
            system_prompt: None,
            messages: vec![Message::Assistant {
                content: vec![ContentBlock::Thinking {
                    thinking: "hmm".into(),
                    thinking_signature: Some("sig".into()),
                }],
                api: String::new(),
                provider: String::new(),
                model: "m".into(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
                duration_ms: None,
            }],
            tools: Vec::new(),
        };
        let msgs = to_rig_messages(&context, Api::AnthropicMessages);
        let RigMessage::Assistant { content, .. } = &msgs[0] else {
            panic!("expected assistant");
        };
        let AssistantContent::Reasoning(sealed) = &content[0] else {
            panic!("expected reasoning");
        };
        assert_eq!(sealed.issuer().as_str(), "anthropic");
        assert!(sealed.open(sealed.issuer()).is_some());
    }
}
