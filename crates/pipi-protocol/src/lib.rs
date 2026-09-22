//! Pipi 的持久化与跨宿主协议类型。
//!
//! 这里的类型同时服务于 session JSONL、Tauri IPC、Web transport 和前端镜像。
//! 不放文件系统、provider 或工具实现，避免协议层反向牵引运行时。唯一的运行
//! 控制原语是轻量的 `AbortSignal`，供 provider、harness 与工具共同使用。

use pipi_error::{ErrorCode, PipiError, RetryHint};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 生成 session/message 的毫秒时间戳。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 支持的 LLM API 协议。
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
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAICompletions => "openai-completions",
        }
    }

    /// OpenAI 兼容协议的 `prompt_tokens` 已包含缓存；Anthropic 的 input
    /// tokens 不包含缓存。provider 适配层据此把用量归一化为 Pipi/pi 口径。
    pub fn prompt_tokens_include_cache(&self) -> bool {
        matches!(self, Self::OpenAICompletions)
    }
}

/// 助手消息内容块。JSON 标签与 pi 保持兼容。
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

/// 工具结果内容块。
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

/// 单次调用的 token 用量。`input` 只计未命中缓存的提示词 token。
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
    pub fn prompt_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    pub fn total(&self) -> u64 {
        self.prompt_tokens() + self.output
    }
}

/// 助手轮次的停止原因。
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

/// 用户、助手和工具结果消息；它是 session JSONL 的核心载荷。
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
            Self::User { .. } => "user",
            Self::Assistant { .. } => "assistant",
            Self::ToolResult { .. } => "toolResult",
        }
    }

    pub fn timestamp(&self) -> u64 {
        match self {
            Self::User { timestamp, .. }
            | Self::Assistant { timestamp, .. }
            | Self::ToolResult { timestamp, .. } => *timestamp,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User {
            content: text.into(),
            timestamp: now_millis(),
        }
    }

    pub fn assistant_text(text: impl Into<String>, model: &str) -> Self {
        Self::Assistant {
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

    pub fn assistant_error(message: impl Into<String>, model: &str, reason: StopReason) -> Self {
        Self::Assistant {
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

    pub fn tool_calls(&self) -> Vec<&ContentBlock> {
        match self {
            Self::Assistant { content, .. } => content
                .iter()
                .filter(|block| matches!(block, ContentBlock::ToolCall { .. }))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// 发给 LLM 的工具定义 wire 格式。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 模型描述。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// 一次 LLM 请求的完整 wire context。
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
}

/// Provider 流式事件。最终消息仍使用稳定的 `Message` 类型。
///
/// 这是一份运行时 transport 契约：provider 只写入事件和重试事实，agent loop
/// 独占是否重发的决策，避免两层重试相乘。
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
        retry: Option<RetryHint>,
    },
}

/// 单次 provider 请求的运行时参数。
#[derive(Debug, Clone)]
pub struct StreamOptions {
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// 无进展超时秒数。0 表示 provider 默认值。
    pub timeout_secs: u64,
    /// 供应商可选的会话路由标识。
    pub session_id: Option<String>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            temperature: None,
            max_tokens: None,
            timeout_secs: 300,
            session_id: None,
        }
    }
}

/// 协作式中止信号。它没有持久化或 IPC 语义，但必须由 provider、harness 与
/// 工具共享，因而和消息协议一起放在最底层的公共 crate。
#[derive(Clone, Default)]
pub struct AbortSignal(Arc<AtomicBool>);

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub async fn wait_aborted(&self) {
        loop {
            if self.is_aborted() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

/// 可由 Web/IPC 暴露的安全错误结构。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorEnvelope {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl From<&PipiError> for ErrorEnvelope {
    fn from(error: &PipiError) -> Self {
        Self {
            code: error.code(),
            message: error.user_message().to_string(),
            retry_after_ms: error.retry_hint().and_then(|hint| hint.after_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_wire_shape_keeps_pi_tags() {
        let message = Message::ToolResult {
            tool_call_id: "call-1".into(),
            tool_name: "read".into(),
            content: vec![ToolResultContent::Text { text: "ok".into() }],
            is_error: false,
            details: None,
            timestamp: 42,
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], "toolResult");
        assert_eq!(json["toolCallId"], "call-1");
        assert_eq!(json["content"][0]["type"], "text");
    }

    #[test]
    fn error_envelope_exposes_only_safe_error_fields() {
        let error = PipiError::retryable(
            ErrorCode::RateLimited,
            "请求过于频繁，请稍后重试",
            pipi_error::RetryHint::after(1_000),
        );
        let envelope = ErrorEnvelope::from(&error);
        assert_eq!(envelope.code, ErrorCode::RateLimited);
        assert_eq!(envelope.retry_after_ms, Some(1_000));
    }

    #[test]
    fn abort_signal_can_be_reused_for_the_next_turn() {
        let signal = AbortSignal::new();
        signal.abort();
        assert!(signal.is_aborted());
        signal.reset();
        assert!(!signal.is_aborted());
    }
}
