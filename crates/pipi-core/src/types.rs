//! 消息 / 内容块 / 流式事件协议。
//!
//! 移植自 `packages/ai/src/types.ts`。JSON 字段名与 pi 保持一致（camelCase、
//! `role` / `type` 作标签），会话文件因此可以对照上游格式阅读。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 支持的 LLM API 协议。移植自 pi 的 `KnownApi`，只保留两个最常用的。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Api {
    #[serde(rename = "anthropic-messages")]
    #[default]
    AnthropicMessages,
    #[serde(rename = "openai-completions")]
    OpenAICompletions,
}

impl Api {
    pub fn as_str(&self) -> &'static str {
        match self {
            Api::AnthropicMessages => "anthropic-messages",
            Api::OpenAICompletions => "openai-completions",
        }
    }
}

/// 助手消息的内容块。对应 pi 的 `TextContent | ThinkingContent | ToolCall`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

/// 工具结果内容块。对应 pi 的 `TextContent | ImageContent`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolResultContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// 对应 pi 的 `StopReason`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    #[default]
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// 三种 LLM 消息：用户 / 助手 / 工具结果。对应 pi 的 `Message`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "role",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Message {
    User {
        content: String,
        timestamp: u64,
    },
    Assistant {
        content: Vec<ContentBlock>,
        api: String,
        provider: String,
        model: String,
        usage: Usage,
        stop_reason: StopReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
        timestamp: u64,
        /// 本次生成耗时（毫秒）；provider 在 Done 时上报，用于 tok/s 统计
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<ToolResultContent>,
        #[serde(default)]
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        timestamp: u64,
    },
}

impl Message {
    pub fn role(&self) -> &'static str {
        match self {
            Message::User { .. } => "user",
            Message::Assistant { .. } => "assistant",
            Message::ToolResult { .. } => "toolResult",
        }
    }

    pub fn timestamp(&self) -> u64 {
        match self {
            Message::User { timestamp, .. }
            | Message::Assistant { timestamp, .. }
            | Message::ToolResult { timestamp, .. } => *timestamp,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Message {
        Message::User {
            content: text.into(),
            timestamp: now_millis(),
        }
    }

    pub fn assistant_text(text: impl Into<String>, model: &str) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::Text { text: text.into() }],
            api: String::new(),
            provider: String::new(),
            model: model.to_string(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: now_millis(),
            duration_ms: None,
        }
    }

    /// 错误 / 中止的助手消息（对应 pi 的 stopReason "error" / "aborted"）。
    pub fn assistant_error(message: impl Into<String>, model: &str, reason: StopReason) -> Message {
        Message::Assistant {
            content: Vec::new(),
            api: String::new(),
            provider: String::new(),
            model: model.to_string(),
            usage: Usage::default(),
            stop_reason: reason,
            error_message: Some(message.into()),
            timestamp: now_millis(),
            duration_ms: None,
        }
    }

    /// 助手消息中的所有工具调用块。
    pub fn tool_calls(&self) -> Vec<&ContentBlock> {
        match self {
            Message::Assistant { content, .. } => content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolCall { .. }))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// 发给 LLM 的工具定义（wire 格式）。对应 pi 的 `Tool`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 模型描述。对应 pi 的 `Model`（简化：去掉 cost / catalog 元数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub api: Api,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// 上下文窗口（token）；0 表示未知（统计里的 context 占比将省略）
    #[serde(default)]
    pub context_window: u64,
}

fn default_max_tokens() -> u32 {
    8192
}

impl Model {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// 一次 LLM 请求的完整上下文。对应 pi 的 `Context`。
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
}

/// Provider 流式事件协议。对应 pi 的 `AssistantMessageEvent`（简化：
/// 不携带 partial 快照，由 agent loop 自行累积；最终消息在 `Done`/`Error` 给出）。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Start,
    TextDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ToolCallStart {
        content_index: usize,
        id: String,
        name: String,
    },
    ToolCallDelta {
        content_index: usize,
        delta: String,
    },
    ToolCallEnd {
        content_index: usize,
        call: ContentBlock,
    },
    Done {
        reason: StopReason,
        usage: Usage,
        message: Box<Message>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct StreamOptions {
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// 单次请求的整体超时（秒）；0 表示使用默认 300 秒。
    pub timeout_secs: u64,
}

impl Default for StreamOptions {
    fn default() -> Self {
        StreamOptions {
            api_key: None,
            temperature: None,
            max_tokens: None,
            timeout_secs: 300,
        }
    }
}

/// 协作式中止信号（对应 pi 的 AbortSignal）。
#[derive(Clone, Default)]
pub struct AbortSignal(Arc<AtomicBool>);

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// 为下一轮复用同一会话时清除上一轮的中止状态。
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// 等待直到被中止（轮询实现，够用即可）。
    pub async fn wait_aborted(&self) {
        loop {
            if self.is_aborted() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AbortSignal;

    #[test]
    fn abort_signal_can_be_reset_for_next_turn() {
        let signal = AbortSignal::new();
        signal.abort();
        assert!(signal.is_aborted());

        signal.reset();

        assert!(!signal.is_aborted());
    }
}
