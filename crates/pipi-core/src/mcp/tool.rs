//! `AgentTool` 适配器：注册名、描述、参数 schema 与结果映射。
//!
//! 服务器的描述与 schema 是外部文本：描述截断后再进提示词，schema 过大时降级
//! 成宽松对象（参数仍原样转发，只是不再做 `required` 轻校验）。

use async_trait::async_trait;
use pipi_mcp::{CallToolResult, ContentPart};
use pipi_protocol::ToolResultContent;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};
use serde_json::Value;

use super::manager::{McpManager, McpToolSpec};

/// 供应商对工具名的限制（Anthropic：`^[a-zA-Z0-9_-]{1,64}$`）。
const MAX_TOOL_NAME: usize = 64;
/// 描述进提示词的字符上限。
const MAX_DESCRIPTION_CHARS: usize = 1024;
/// 参数 schema 的字节上限；超过就降级，不让单个服务器吃掉上下文。
const MAX_SCHEMA_BYTES: usize = 16 * 1024;

/// `mcp__<server>__<tool>`，字符集外的一律换成 `_`，超长时截断并追加稳定哈希。
pub fn wire_name(server: &str, tool: &str) -> String {
    let name = format!("mcp__{}__{}", sanitize(server), sanitize(tool));
    if name.len() <= MAX_TOOL_NAME {
        return name;
    }
    // sanitize 之后全是 ASCII，按字节截断是安全的。
    let mut truncated = name[..MAX_TOOL_NAME - 9].to_string();
    truncated.push('_');
    truncated.push_str(&short_hash(&name));
    truncated
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn short_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut text: String = value.chars().take(max).collect();
    text.push('…');
    text
}

pub struct McpTool {
    manager: McpManager,
    agent_name: String,
    spec: McpToolSpec,
}

impl McpTool {
    pub fn new(manager: McpManager, agent_name: String, spec: McpToolSpec) -> Self {
        Self {
            manager,
            agent_name,
            spec,
        }
    }
}

#[async_trait]
impl AgentTool for McpTool {
    fn name(&self) -> &str {
        &self.spec.wire_name
    }

    fn description(&self) -> String {
        let detail = self
            .spec
            .tool
            .description
            .as_deref()
            .or(self.spec.tool.title.as_deref())
            .unwrap_or("No description provided by the server");
        truncate_chars(
            &format!("[MCP server: {}] {}", self.spec.server, detail.trim()),
            MAX_DESCRIPTION_CHARS,
        )
    }

    fn parameters(&self) -> Value {
        let schema = &self.spec.tool.input_schema;
        match serde_json::to_string(schema) {
            Ok(text) if text.len() <= MAX_SCHEMA_BYTES => schema.clone(),
            _ => serde_json::json!({
                "type": "object",
                "additionalProperties": true,
                "description": "(parameter schema omitted: it exceeds the size limit)",
            }),
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let result = self
            .manager
            .call(
                &self.agent_name,
                &self.spec.server,
                &self.spec.tool.name,
                args.clone(),
            )
            .await?;
        render(result)
    }
}

/// MCP 的 content 数组 → `ToolOutput`；服务器自报的 `isError` 走错误结果。
fn render(result: CallToolResult) -> Result<ToolOutput, String> {
    let mut content = Vec::with_capacity(result.content.len());
    let mut texts = Vec::new();
    for part in result.content {
        match part {
            ContentPart::Text(text) => {
                texts.push(text.clone());
                content.push(ToolResultContent::Text { text });
            }
            ContentPart::Image { data, mime_type } => {
                content.push(ToolResultContent::Image { data, mime_type })
            }
            // 文本资源把 uri 放在前一行，模型能引用来源；二进制 resource 走 Other。
            ContentPart::ResourceText { uri, text, .. } => {
                let text = format!("[{uri}]\n{text}");
                texts.push(text.clone());
                content.push(ToolResultContent::Text { text });
            }
            ContentPart::Other(value) => {
                let text = value.to_string();
                texts.push(text.clone());
                content.push(ToolResultContent::Text { text });
            }
        }
    }
    if content.is_empty()
        && let Some(structured) = &result.structured
    {
        let text =
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string());
        content.push(ToolResultContent::Text { text });
    }
    if content.is_empty() {
        content.push(ToolResultContent::Text {
            text: "(the MCP tool returned no content)".into(),
        });
    }
    if result.is_error {
        let message = if texts.is_empty() {
            "(the MCP tool reported an error without output)".to_string()
        } else {
            texts.join("\n")
        };
        return Err(message);
    }
    Ok(ToolOutput {
        content,
        details: result.structured,
        terminate: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_stay_within_the_provider_limit_and_are_stable() {
        assert_eq!(wire_name("fs", "read_file"), "mcp__fs__read_file");
        // 非安全字符替换掉，不会带出路径或标点。
        assert_eq!(wire_name("a.b", "x/y"), "mcp__a_b__x_y");
        let long_tool = "t".repeat(120);
        let name = wire_name("server", &long_tool);
        assert!(name.len() <= MAX_TOOL_NAME);
        assert_eq!(name, wire_name("server", &long_tool));
        assert_ne!(name, wire_name("server", &format!("{long_tool}2")));
    }

    #[test]
    fn render_maps_text_image_and_errors() {
        let text = render(CallToolResult {
            content: vec![ContentPart::Text("ok".into())],
            structured: None,
            is_error: false,
        })
        .unwrap();
        assert!(matches!(&text.content[0], ToolResultContent::Text { text } if text == "ok"));

        let image = render(CallToolResult {
            content: vec![ContentPart::Image {
                data: "aGk=".into(),
                mime_type: "image/png".into(),
            }],
            structured: None,
            is_error: false,
        })
        .unwrap();
        assert!(matches!(
            &image.content[0],
            ToolResultContent::Image { data, .. } if data == "aGk="
        ));

        let error = render(CallToolResult {
            content: vec![ContentPart::Text("boom".into())],
            structured: None,
            is_error: true,
        })
        .unwrap_err();
        assert_eq!(error, "boom");
    }

    #[test]
    fn structured_only_results_are_rendered_as_text() {
        let output = render(CallToolResult {
            content: Vec::new(),
            structured: Some(serde_json::json!({ "count": 2 })),
            is_error: false,
        })
        .unwrap();
        assert!(
            matches!(&output.content[0], ToolResultContent::Text { text } if text.contains("count"))
        );
        assert_eq!(output.details.unwrap()["count"], 2);
    }
}
