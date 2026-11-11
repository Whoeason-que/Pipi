//! MCP 的 wire 形状：只放需要的子集（tools），解析时对未知字段与未知内容
//! 类型保持宽容 —— 服务器实现参差，一个不认识的内容块不该让整次调用失败。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 客户端声明的协议版本（2025-06-18）。服务器可以回一个更旧的版本，按它回的走。
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// 低于这个版本的服务器不支持 `tools` 的当前形状，握手直接失败。
pub(crate) const MIN_PROTOCOL_VERSION: &str = "2024-11-05";

/// 服务器返回的一个工具。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// 已经是 JSON Schema，宿主可以直接当工具参数 schema 用。
    #[serde(default = "object_schema")]
    pub input_schema: Value,
}

fn object_schema() -> Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

/// 一次 `tools/call` 的结果。
#[derive(Debug, Clone, Default)]
pub struct CallToolResult {
    pub content: Vec<ContentPart>,
    /// 服务器声明的结构化结果（我们有 tool result 的 `details` 可放）。
    pub structured: Option<Value>,
    /// 服务器自报的失败：工具跑了但业务失败，应当作为错误结果交给模型。
    pub is_error: bool,
}

/// 结果内容块。只区分模型能消费的几种；不认识的原样保留给宿主决定怎么展示。
#[derive(Debug, Clone)]
pub enum ContentPart {
    Text(String),
    Image { data: String, mime_type: String },
    ResourceText { uri: String, text: String },
    Other(Value),
}

/// `initialize` 握手里服务器自报的身份。
#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
    pub protocol_version: String,
}

/// 一条入站消息。响应、服务器发来的请求、通知三者靠 `id`/`method` 区分。
#[derive(Debug, Deserialize)]
pub(crate) struct Inbound {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
}

/// 把 `tools/call` 的结果值解析成 [`CallToolResult`]。
pub(crate) fn parse_call_result(value: &Value) -> CallToolResult {
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_content).collect())
        .unwrap_or_default();
    CallToolResult {
        content,
        structured: value.get("structuredContent").cloned(),
        is_error: value
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_content(value: &Value) -> ContentPart {
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => ContentPart::Text(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "image" => ContentPart::Image {
            data: value
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            mime_type: value
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string(),
        },
        "resource" => {
            let resource = value.get("resource").unwrap_or(&Value::Null);
            match resource.get("text").and_then(Value::as_str) {
                Some(text) => ContentPart::ResourceText {
                    uri: resource
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    text: text.to_string(),
                },
                // 二进制 resource（base64 blob）先原样交给宿主，不在这里猜怎么展示。
                None => ContentPart::Other(value.clone()),
            }
        }
        _ => ContentPart::Other(value.clone()),
    }
}
