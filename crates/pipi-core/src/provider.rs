//! LLM provider 层：Anthropic Messages 与 OpenAI Chat Completions 的流式适配。
//!
//! 移植自 `packages/ai/src/api/anthropic-messages.ts` 与
//! `openai-completions.ts`（只保留核心流式路径；重试、缓存标记、thinking
//! 配置等见 lib.rs 的「尚未移植」清单）。错误不在 HTTP 层抛出，而是以
//! `StreamEvent::Error` 进入事件流 —— 与 pi 的 StreamFn 契约一致。

use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::{self, Sender};

use crate::types::{
    AbortSignal, Api, ContentBlock, Context, Message, Model, StopReason, StreamEvent, StreamOptions,
    ToolResultContent, Usage,
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
    match api {
        Api::AnthropicMessages => std::sync::Arc::new(AnthropicProvider::new()),
        Api::OpenAICompletions => std::sync::Arc::new(OpenAIProvider::new()),
    }
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client")
}

fn trim_base(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

fn send_error(tx: &Sender<StreamEvent>, message: String) {
    let _ = tx.try_send(StreamEvent::Error { message });
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// SSE 消费循环：按行读取 `data:` 载荷交给 `on_data`；事件统一在此发送。
/// `on_data` 返回 true 表示流已终结（Done/Error 已入队）。
async fn run_sse(
    response: reqwest::Response,
    abort: AbortSignal,
    idle_secs: u64,
    tx: Sender<StreamEvent>,
    mut on_data: impl FnMut(&str) -> (Vec<StreamEvent>, bool) + Send,
) {
    let idle = Duration::from_secs(if idle_secs == 0 { 300 } else { idle_secs });
    let mut stream = std::pin::pin!(response.bytes_stream());
    let mut buf: Vec<u8> = Vec::new();

    loop {
        let chunk = tokio::select! {
            _ = abort.wait_aborted() => None,
            item = tokio::time::timeout(idle, stream.next()) => match item {
                Err(_) => {
                    let _ = tx.send(StreamEvent::Error { message: "流式响应空闲超时".into() }).await;
                    return;
                }
                Ok(None) => None,
                Ok(Some(Err(e))) => {
                    let _ = tx.send(StreamEvent::Error { message: format!("网络错误: {e}") }).await;
                    return;
                }
                Ok(Some(Ok(bytes))) => Some(bytes),
            },
        };

        match chunk {
            None => return, // 中止或流结束（正常结束路径一定经过 terminal）
            Some(bytes) => {
                buf.extend_from_slice(&bytes);
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim_end_matches(['\n', '\r']);
                    let Some(payload) = line.strip_prefix("data:") else {
                        continue; // event: / 注释行
                    };
                    let payload = payload.trim();
                    if payload.is_empty() {
                        continue;
                    }
                    let (events, terminal) = on_data(payload);
                    for ev in events {
                        if tx.send(ev).await.is_err() {
                            return; // 接收端已放弃（例如被中止）
                        }
                    }
                    if terminal {
                        return;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Anthropic Messages
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        AnthropicProvider {
            client: build_client(),
        }
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 Context 转成 Anthropic messages 数组。连续同角色消息合并为一条
/// （多个 tool_result 块进入同一个 user 消息），与 pi 的行为一致。
pub fn anthropic_messages(context: &Context) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut push = |role: &str, blocks: Vec<Value>| {
        if let Some(last) = out.last_mut() {
            if last["role"] == json!(role) {
                if let Some(arr) = last["content"].as_array_mut() {
                    arr.extend(blocks);
                    return;
                }
            }
        }
        out.push(json!({ "role": role, "content": blocks }));
    };

    for msg in &context.messages {
        match msg {
            Message::User { content, .. } => {
                push("user", vec![json!({ "type": "text", "text": content })]);
            }
            Message::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content.iter().map(anthropic_result_block).collect::<Vec<_>>(),
                    "is_error": is_error,
                });
                push("user", vec![block]);
            }
            Message::Assistant { content, .. } => {
                let blocks: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => {
                            Some(json!({ "type": "text", "text": text }))
                        }
                        ContentBlock::Thinking {
                            thinking,
                            thinking_signature,
                        } => thinking_signature.as_ref().map(|sig| {
                            json!({ "type": "thinking", "thinking": thinking, "signature": sig })
                        }),
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => Some(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": arguments,
                        })),
                    })
                    .collect();
                if !blocks.is_empty() {
                    push("assistant", blocks);
                }
            }
        }
    }
    out
}

fn anthropic_result_block(c: &ToolResultContent) -> Value {
    match c {
        ToolResultContent::Text { text } => json!({ "type": "text", "text": text }),
        ToolResultContent::Image { data, mime_type } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": mime_type, "data": data },
        }),
    }
}

pub fn anthropic_payload(model: &Model, context: &Context, options: &StreamOptions) -> Value {
    let mut payload = json!({
        "model": model.id,
        "max_tokens": options.max_tokens.unwrap_or(model.max_tokens),
        "messages": anthropic_messages(context),
        "stream": true,
    });
    if let Some(system) = &context.system_prompt {
        payload["system"] = json!(system);
    }
    if !context.tools.is_empty() {
        payload["tools"] = Value::Array(
            context
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect(),
        );
    }
    if let Some(temp) = options.temperature {
        payload["temperature"] = json!(temp);
    }
    payload
}

fn map_anthropic_stop(reason: &str) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::Length,
        _ => StopReason::Stop, // end_turn / stop_sequence
    }
}

enum BlockAcc {
    Text(String),
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        json: String,
    },
}

/// Anthropic SSE 累积状态。每个 data 事件产出零或多个 StreamEvent。
struct AnthropicStream {
    model_id: String,
    blocks: std::collections::BTreeMap<usize, BlockAcc>,
    usage: Usage,
    stop_reason: StopReason,
    started: bool,
}

impl AnthropicStream {
    fn new(model_id: String) -> Self {
        AnthropicStream {
            model_id,
            blocks: Default::default(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            started: false,
        }
    }

    /// 把累积块按序转成最终助手消息。
    fn finalize(&mut self) -> Message {
        let content: Vec<ContentBlock> = self
            .blocks
            .values()
            .map(|acc| match acc {
                BlockAcc::Text(text) => ContentBlock::Text { text: text.clone() },
                BlockAcc::Thinking { thinking, signature } => ContentBlock::Thinking {
                    thinking: thinking.clone(),
                    thinking_signature: signature.clone(),
                },
                BlockAcc::Tool { id, name, json } => ContentBlock::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: if json.is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(json).unwrap_or(json!({}))
                    },
                },
            })
            .collect();
        let mut usage = self.usage;
        usage.total_tokens = usage.total();
        Message::Assistant {
            content,
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: self.model_id.clone(),
            usage,
            stop_reason: self.stop_reason,
            error_message: None,
            timestamp: crate::types::now_millis(),
        }
    }

    fn on_data(&mut self, data: &str) -> (Vec<StreamEvent>, bool) {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return (Vec::new(), false);
        };
        match v["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.started = true;
                let usage = &v["message"]["usage"];
                self.usage.input = usage["input_tokens"].as_u64().unwrap_or(0);
                self.usage.cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                self.usage.cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                (vec![StreamEvent::Start], false)
            }
            "content_block_start" => {
                let index = v["index"].as_u64().unwrap_or(0) as usize;
                let block = &v["content_block"];
                match block["type"].as_str().unwrap_or("") {
                    "tool_use" => {
                        self.blocks.insert(
                            index,
                            BlockAcc::Tool {
                                id: block["id"].as_str().unwrap_or("").to_string(),
                                name: block["name"].as_str().unwrap_or("").to_string(),
                                json: String::new(),
                            },
                        );
                    }
                    "thinking" => {
                        self.blocks.insert(
                            index,
                            BlockAcc::Thinking {
                                thinking: String::new(),
                                signature: None,
                            },
                        );
                    }
                    _ => {
                        self.blocks.insert(index, BlockAcc::Text(String::new()));
                    }
                }
                (Vec::new(), false)
            }
            "content_block_delta" => {
                let index = v["index"].as_u64().unwrap_or(0) as usize;
                let delta = &v["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or("");
                        if let Some(BlockAcc::Text(t)) = self.blocks.get_mut(&index) {
                            t.push_str(text);
                        }
                        (
                            vec![StreamEvent::TextDelta {
                                content_index: index,
                                delta: text.to_string(),
                            }],
                            false,
                        )
                    }
                    "thinking_delta" => {
                        let thinking = delta["thinking"].as_str().unwrap_or("");
                        if let Some(BlockAcc::Thinking { thinking: t, .. }) =
                            self.blocks.get_mut(&index)
                        {
                            t.push_str(thinking);
                        }
                        (
                            vec![StreamEvent::ThinkingDelta {
                                content_index: index,
                                delta: thinking.to_string(),
                            }],
                            false,
                        )
                    }
                    "signature_delta" => {
                        if let Some(BlockAcc::Thinking { signature, .. }) =
                            self.blocks.get_mut(&index)
                        {
                            let sig = signature.get_or_insert_with(String::new);
                            sig.push_str(delta["signature"].as_str().unwrap_or(""));
                        }
                        (Vec::new(), false)
                    }
                    "input_json_delta" => {
                        let partial = delta["partial_json"].as_str().unwrap_or("");
                        if let Some(BlockAcc::Tool { json, .. }) = self.blocks.get_mut(&index) {
                            json.push_str(partial);
                        }
                        (
                            vec![StreamEvent::ToolCallDelta {
                                content_index: index,
                                delta: partial.to_string(),
                            }],
                            false,
                        )
                    }
                    _ => (Vec::new(), false),
                }
            }
            "content_block_stop" => {
                let index = v["index"].as_u64().unwrap_or(0) as usize;
                if let Some(BlockAcc::Tool { id, name, json }) = self.blocks.get(&index) {
                    let arguments: Value = if json.is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(json).unwrap_or(json!({}))
                    };
                    return (
                        vec![StreamEvent::ToolCallEnd {
                            content_index: index,
                            call: ContentBlock::ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments,
                            },
                        }],
                        false,
                    );
                }
                (Vec::new(), false)
            }
            "message_delta" => {
                self.usage.output = v["usage"]["output_tokens"].as_u64().unwrap_or(self.usage.output);
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = map_anthropic_stop(reason);
                }
                (Vec::new(), false)
            }
            "message_stop" => {
                let message = self.finalize();
                let reason = match &message {
                    Message::Assistant { stop_reason, .. } => *stop_reason,
                    _ => StopReason::Stop,
                };
                let usage = match &message {
                    Message::Assistant { usage, .. } => *usage,
                    _ => Usage::default(),
                };
                (
                    vec![StreamEvent::Done {
                        reason,
                        usage,
                        message: Box::new(message),
                    }],
                    true,
                )
            }
            "error" => {
                let msg = v["error"]["message"]
                    .as_str()
                    .unwrap_or("provider error")
                    .to_string();
                (vec![StreamEvent::Error { message: msg }], true)
            }
            _ => (Vec::new(), false),
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        abort: AbortSignal,
    ) -> EventStream {
        let (tx, rx) = mpsc::channel(256);
        let payload = anthropic_payload(model, context, options);
        let url = format!("{}/v1/messages", trim_base(&model.base_url));
        let client = self.client.clone();
        let api_key = options.api_key.clone();
        let idle = options.timeout_secs;
        let model_id = model.id.clone();

        tokio::spawn(async move {
            let mut req = client
                .post(url)
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01");
            if let Some(key) = &api_key {
                req = req.header("x-api-key", key);
            }
            match req.json(&payload).send().await {
                Err(e) => send_error(&tx, format!("请求失败: {e}")),
                Ok(resp) if !resp.status().is_success() => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    send_error(&tx, format!("HTTP {status}: {}", truncate_str(&body, 2000)));
                }
                Ok(resp) => {
                    let mut st = AnthropicStream::new(model_id);
                    run_sse(resp, abort, idle, tx, move |data| st.on_data(data)).await;
                }
            }
        });
        rx
    }
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions（兼容协议）
// ---------------------------------------------------------------------------

pub struct OpenAIProvider {
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new() -> Self {
        OpenAIProvider {
            client: build_client(),
        }
    }
}

impl Default for OpenAIProvider {
    fn default() -> Self {
        Self::new()
    }
}

pub fn openai_messages(context: &Context) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if let Some(system) = &context.system_prompt {
        out.push(json!({ "role": "system", "content": system }));
    }
    for msg in &context.messages {
        match msg {
            Message::User { content, .. } => {
                out.push(json!({ "role": "user", "content": content }));
            }
            Message::Assistant { content, .. } => {
                let text: String = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let tool_calls: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": serde_json::to_string(arguments).unwrap_or_default(),
                            },
                        })),
                        _ => None,
                    })
                    .collect();
                let mut m = serde_json::Map::new();
                m.insert("role".into(), json!("assistant"));
                if text.is_empty() && !tool_calls.is_empty() {
                    m.insert("content".into(), Value::Null);
                } else {
                    m.insert("content".into(), json!(text));
                }
                if !tool_calls.is_empty() {
                    m.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                out.push(Value::Object(m));
            }
            Message::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                let text: String = content
                    .iter()
                    .map(|b| match b {
                        ToolResultContent::Text { text } => text.as_str(),
                        ToolResultContent::Image { .. } => "[image]",
                    })
                    .collect::<Vec<_>>()
                    .join("");
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": text,
                }));
            }
        }
    }
    out
}

pub fn openai_payload(model: &Model, context: &Context, options: &StreamOptions) -> Value {
    let mut payload = json!({
        "model": model.id,
        "messages": openai_messages(context),
        "max_tokens": options.max_tokens.unwrap_or(model.max_tokens),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !context.tools.is_empty() {
        payload["tools"] = Value::Array(
            context
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        },
                    })
                })
                .collect(),
        );
    }
    if let Some(temp) = options.temperature {
        payload["temperature"] = json!(temp);
    }
    payload
}

struct ToolAcc {
    id: String,
    name: String,
    args: String,
}

/// OpenAI SSE 累积状态。
struct OpenAIStream {
    model_id: String,
    text: String,
    thinking: String,
    tools: Vec<ToolAcc>,
    finish: Option<String>,
    usage: Usage,
    started: bool,
}

impl OpenAIStream {
    fn new(model_id: String) -> Self {
        OpenAIStream {
            model_id,
            text: String::new(),
            thinking: String::new(),
            tools: Vec::new(),
            finish: None,
            usage: Usage::default(),
            started: false,
        }
    }

    fn finalize(&mut self) -> (Vec<StreamEvent>, bool) {
        let mut events: Vec<StreamEvent> = Vec::new();
        let mut content: Vec<ContentBlock> = Vec::new();
        if !self.thinking.is_empty() {
            content.push(ContentBlock::Thinking {
                thinking: std::mem::take(&mut self.thinking),
                thinking_signature: None,
            });
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::Text {
                text: std::mem::take(&mut self.text),
            });
        }
        let tools = std::mem::take(&mut self.tools);
        for (index, acc) in tools.iter().enumerate() {
            let arguments: Value = if acc.args.is_empty() {
                json!({})
            } else {
                serde_json::from_str(&acc.args).unwrap_or(json!({}))
            };
            events.push(StreamEvent::ToolCallEnd {
                content_index: index,
                call: ContentBlock::ToolCall {
                    id: acc.id.clone(),
                    name: acc.name.clone(),
                    arguments: arguments.clone(),
                },
            });
            content.push(ContentBlock::ToolCall {
                id: acc.id.clone(),
                name: acc.name.clone(),
                arguments,
            });
        }
        let reason = match self.finish.as_deref() {
            Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
            Some("length") | Some("max_tokens") => StopReason::Length,
            _ => {
                if tools.iter().any(|t| !t.name.is_empty()) {
                    StopReason::ToolUse
                } else {
                    StopReason::Stop
                }
            }
        };
        let mut usage = self.usage;
        usage.total_tokens = usage.total();
        let message = Message::Assistant {
            content,
            api: "openai-completions".into(),
            provider: "openai-compatible".into(),
            model: self.model_id.clone(),
            usage,
            stop_reason: reason,
            error_message: None,
            timestamp: crate::types::now_millis(),
        };
        events.push(StreamEvent::Done {
            reason,
            usage,
            message: Box::new(message),
        });
        (events, true)
    }

    fn on_data(&mut self, data: &str) -> (Vec<StreamEvent>, bool) {
        if data.trim() == "[DONE]" {
            return self.finalize();
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return (Vec::new(), false);
        };
        let mut events: Vec<StreamEvent> = Vec::new();
        if !self.started {
            self.started = true;
            events.push(StreamEvent::Start);
        }
        if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
            self.usage.input = usage["prompt_tokens"].as_u64().unwrap_or(0);
            self.usage.output = usage["completion_tokens"].as_u64().unwrap_or(0);
        }
        let Some(choice) = v["choices"].get(0) else {
            return (events, false);
        };
        let delta = &choice["delta"];
        if let Some(reasoning) = delta["reasoning_content"].as_str() {
            self.thinking.push_str(reasoning);
            events.push(StreamEvent::ThinkingDelta {
                content_index: 0,
                delta: reasoning.to_string(),
            });
        }
        if let Some(text) = delta["content"].as_str() {
            self.text.push_str(text);
            events.push(StreamEvent::TextDelta {
                content_index: 1,
                delta: text.to_string(),
            });
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                while self.tools.len() <= index {
                    self.tools.push(ToolAcc {
                        id: String::new(),
                        name: String::new(),
                        args: String::new(),
                    });
                }
                let acc = &mut self.tools[index];
                if let Some(id) = call["id"].as_str() {
                    acc.id = id.to_string();
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    acc.name = name.to_string();
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    acc.args.push_str(args);
                    events.push(StreamEvent::ToolCallDelta {
                        content_index: index,
                        delta: args.to_string(),
                    });
                }
            }
        }
        if let Some(finish) = choice["finish_reason"].as_str() {
            self.finish = Some(finish.to_string());
        }
        (events, false)
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        abort: AbortSignal,
    ) -> EventStream {
        let (tx, rx) = mpsc::channel(256);
        let payload = openai_payload(model, context, options);
        let url = format!("{}/chat/completions", trim_base(&model.base_url));
        let client = self.client.clone();
        let api_key = options.api_key.clone();
        let idle = options.timeout_secs;
        let model_id = model.id.clone();

        tokio::spawn(async move {
            let mut req = client.post(url).header("content-type", "application/json");
            if let Some(key) = &api_key {
                req = req.header("authorization", format!("Bearer {key}"));
            }
            match req.json(&payload).send().await {
                Err(e) => send_error(&tx, format!("请求失败: {e}")),
                Ok(resp) if !resp.status().is_success() => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    send_error(&tx, format!("HTTP {status}: {}", truncate_str(&body, 2000)));
                }
                Ok(resp) => {
                    let mut st = OpenAIStream::new(model_id);
                    run_sse(resp, abort, idle, tx, move |data| st.on_data(data)).await;
                }
            }
        });
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Api, Tool};

    fn model(api: Api) -> Model {
        Model {
            id: "test-model".into(),
            name: "test".into(),
            api,
            base_url: "https://example.com".into(),
            max_tokens: 1024,
        }
    }

    fn assistant_with_tool_calls() -> Message {
        Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                arguments: json!({"cmd": "ls"}),
            }],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
        }
    }

    #[test]
    fn anthropic_system_is_top_level() {
        let ctx = Context {
            system_prompt: Some("be brief".into()),
            messages: vec![Message::user_text("hello")],
            tools: vec![Tool {
                name: "read".into(),
                description: "d".into(),
                parameters: json!({"type": "object"}),
            }],
        };
        let payload =
            anthropic_payload(&model(Api::AnthropicMessages), &ctx, &StreamOptions::default());
        assert_eq!(payload["system"], json!("be brief"));
        assert_eq!(payload["max_tokens"], json!(1024));
        assert_eq!(payload["messages"][0]["role"], json!("user"));
        assert_eq!(payload["tools"][0]["input_schema"]["type"], json!("object"));
    }

    #[test]
    fn anthropic_merges_consecutive_tool_results() {
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::user_text("do it"),
                assistant_with_tool_calls(),
                Message::ToolResult {
                    tool_call_id: "t1".into(),
                    tool_name: "bash".into(),
                    content: vec![ToolResultContent::Text { text: "out".into() }],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                },
                Message::ToolResult {
                    tool_call_id: "t2".into(),
                    tool_name: "read".into(),
                    content: vec![ToolResultContent::Text { text: "out2".into() }],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                },
            ],
            tools: vec![],
        };
        let msgs = anthropic_messages(&ctx);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2]["role"], json!("user"));
        assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(msgs[2]["content"][0]["type"], json!("tool_result"));
    }

    #[test]
    fn anthropic_replays_tool_use() {
        let ctx = Context {
            system_prompt: None,
            messages: vec![assistant_with_tool_calls()],
            tools: vec![],
        };
        let msgs = anthropic_messages(&ctx);
        assert_eq!(msgs[0]["role"], json!("assistant"));
        let block = &msgs[0]["content"][0];
        assert_eq!(block["type"], json!("tool_use"));
        assert_eq!(block["input"], json!({"cmd": "ls"}));
    }

    #[test]
    fn openai_tool_result_uses_tool_role() {
        let ctx = Context {
            system_prompt: Some("sys".into()),
            messages: vec![
                assistant_with_tool_calls(),
                Message::ToolResult {
                    tool_call_id: "t1".into(),
                    tool_name: "bash".into(),
                    content: vec![ToolResultContent::Text { text: "done".into() }],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                },
            ],
            tools: vec![],
        };
        let payload =
            openai_payload(&model(Api::OpenAICompletions), &ctx, &StreamOptions::default());
        let msgs = payload["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], json!("system"));
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], json!("bash"));
        assert_eq!(msgs[2]["role"], json!("tool"));
        assert_eq!(msgs[2]["tool_call_id"], json!("t1"));
        assert_eq!(msgs[2]["content"], json!("done"));
    }

    #[test]
    fn openai_stream_accumulates_tool_calls() {
        let mut st = OpenAIStream::new("gpt-x".into());
        let chunks = [
            r#"{"choices":[{"delta":{"role":"assistant","content":"hi"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":"{\"c"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"md\":\"ls\"}"}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
            "[DONE]",
        ];
        let mut all: Vec<StreamEvent> = Vec::new();
        for c in chunks {
            let (mut evs, terminal) = st.on_data(c);
            all.append(&mut evs);
            if terminal {
                break;
            }
        }
        let done = all
            .iter()
            .find_map(|e| match e {
                StreamEvent::Done { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("done event");
        let Message::Assistant { content, stop_reason, usage, model, .. } = *done else {
            panic!("expected assistant");
        };
        assert_eq!(stop_reason, StopReason::ToolUse);
        assert_eq!(model, "gpt-x");
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 5);
        assert_eq!(content.len(), 2); // text + toolCall
        assert_eq!(
            content[1],
            ContentBlock::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"cmd": "ls"}),
            }
        );
    }

    #[test]
    fn anthropic_stream_full_lifecycle() {
        let mut st = AnthropicStream::new("claude-x".into());
        let lines = [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":7}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu1","name":"read"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"th\":\"a.md\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let mut all: Vec<StreamEvent> = Vec::new();
        for l in lines {
            let (mut evs, terminal) = st.on_data(l);
            all.append(&mut evs);
            if terminal {
                break;
            }
        }
        let done = all
            .iter()
            .find_map(|e| match e {
                StreamEvent::Done { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("done event");
        let Message::Assistant {
            content, usage, stop_reason, ..
        } = *done
        else {
            panic!("expected assistant");
        };
        assert_eq!(stop_reason, StopReason::ToolUse);
        assert_eq!(usage.input, 7);
        assert_eq!(usage.output, 9);
        assert_eq!(
            content[0],
            ContentBlock::ToolCall {
                id: "tu1".into(),
                name: "read".into(),
                arguments: json!({"path": "a.md"}),
            }
        );
    }
}
