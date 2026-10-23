//! 单元测试共享 fixture。
//!
//! 只放跨多个测试模块重复出现的构造器；单文件专用的助手（如 `tool_call`、
//! `assistant_with_calls`）留在各自文件里。

use pipi_protocol::{ContentBlock, Message, StopReason, ToolResultContent, Usage};

/// 固定 `Stop` 结束、无用量/时间的助手文本消息。
pub(crate) fn assistant_text(text: &str) -> Message {
    Message::Assistant {
        content: vec![ContentBlock::Text { text: text.into() }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        duration_ms: None,
    }
}

/// 成功（`is_error: false`）的工具结果文本消息；`tool_call_id` / `tool_name`
/// 由调用方给（各测试对身份的取值不同，其余字段一致）。
pub(crate) fn tool_result(tool_call_id: &str, tool_name: &str, text: &str) -> Message {
    Message::ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        content: vec![ToolResultContent::Text { text: text.into() }],
        is_error: false,
        details: None,
        timestamp: 0,
    }
}
