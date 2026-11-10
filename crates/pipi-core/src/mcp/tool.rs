//! `AgentTool` 适配器：注册名、描述、参数 schema 与结果映射。
//!
//! 服务器的描述与 schema 是外部文本：描述截断后再进提示词，schema 过大时降级
//! 成宽松对象（参数仍原样转发，只是不再做 `required` 轻校验）。

use async_trait::async_trait;
use pipi_mcp::{CallToolResult, ContentPart};
use pipi_protocol::ToolResultContent;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};
use pipi_tools::truncate::truncate_head;
use serde_json::Value;

use super::manager::{McpManager, McpToolSpec};

/// 供应商对工具名的限制（Anthropic：`^[a-zA-Z0-9_-]{1,64}$`）。
const MAX_TOOL_NAME: usize = 64;
/// 描述进提示词的字符上限。
const MAX_DESCRIPTION_CHARS: usize = 1024;
/// 参数 schema 的字节上限；超过就降级，不让单个服务器吃掉上下文。
pub(super) const MAX_SCHEMA_BYTES: usize = 16 * 1024;
/// `details` 落盘前的字节上限（它是会话条目的一部分，不进提示词）。
const MAX_DETAILS_BYTES: usize = 64 * 1024;

/// `mcp__<server>__<tool>`，字符集外的一律换成 `_`，超长时截断并追加稳定哈希。
pub fn wire_name(server: &str, tool: &str) -> String {
    let name = format!("mcp__{}__{}", sanitize(server), sanitize(tool));
    // Sanitization and the `__` delimiter can otherwise alias distinct names.
    let needs_hash = sanitize(server) != server
        || sanitize(tool) != tool
        || server.contains("__")
        || tool.contains("__")
        || server.ends_with('_')
        || tool.starts_with('_')
        || name.len() > MAX_TOOL_NAME;
    if !needs_hash {
        return name;
    }
    let original = serde_json::to_string(&(server, tool)).expect("names are serializable");
    let prefix = &name[..name.len().min(MAX_TOOL_NAME - 9)];
    format!("{prefix}_{}", short_hash(&original))
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
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let result = self
            .manager
            .call_scoped(
                &self.agent_name,
                &self.spec.server,
                &self.spec.tool.name,
                args.clone(),
                &ctx.abort,
                self.spec.strict,
            )
            .await?;
        render(result)
    }
}

/// MCP 的 content 数组 → `ToolOutput`。
///
/// 文本块聚合成一条结果再统一截断（与 bash 同一套行/字节上限）：服务器返回
/// 一兆字节的日志时，写进会话的是有界的一段，而不是整份。`isError` 走错误
/// 结果；图片原样透传（供应商侧只支持图片块时由 provider 降级成文本）。
fn render(result: CallToolResult) -> Result<ToolOutput, String> {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for part in result.content {
        match part {
            ContentPart::Text(text) => texts.push(text),
            ContentPart::Image { data, mime_type } => {
                const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
                if data.len() > MAX_IMAGE_BYTES {
                    texts.push("[MCP image omitted: image exceeds size limit]".into());
                } else {
                    images.push(ToolResultContent::Image { data, mime_type });
                }
            }
            // 文本资源把 uri 放在前一行，模型能引用来源；二进制 resource 走 Other。
            ContentPart::ResourceText { uri, text, .. } => texts.push(format!("[{uri}]\n{text}")),
            ContentPart::Other(value) => texts.push(value.to_string()),
        }
    }
    // 只有 structuredContent 时也给它一个文本形态，模型才看得到。
    if texts.is_empty()
        && let Some(structured) = &result.structured
    {
        texts.push(
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string()),
        );
    }
    let text = truncate_output(&texts.join("\n\n"));
    if result.is_error {
        return Err(if text.trim().is_empty() {
            "(the MCP tool reported an error without output)".to_string()
        } else {
            text
        });
    }
    let mut content: Vec<ToolResultContent> = Vec::new();
    if !text.is_empty() {
        content.push(ToolResultContent::Text { text });
    }
    content.extend(images);
    if content.is_empty() {
        content.push(ToolResultContent::Text {
            text: "(the MCP tool returned no content)".into(),
        });
    }
    Ok(ToolOutput {
        content,
        // `details` 会原样写进会话条目：超大的结构化结果不落盘。
        details: result
            .structured
            .filter(|value| serialized_len(value) <= MAX_DETAILS_BYTES),
        terminate: false,
    })
}

fn truncate_output(text: &str) -> String {
    let max_bytes = pipi_tools::truncate::DEFAULT_MAX_BYTES;
    let truncated = truncate_head(text, pipi_tools::truncate::DEFAULT_MAX_LINES, max_bytes);
    if !truncated.truncated {
        return truncated.content;
    }
    // 单行自身超限时 `truncate_head` 会整行保留（read 工具自己处理越界），
    // 这里必须真的切到字节上限，否则一个单行大输出就能绕过限制。
    if truncated.first_line_exceeds_limit {
        let mut end = max_bytes.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        return format!(
            "{}\n[…line truncated…]\n\n[output truncated: the first line alone exceeds {max_bytes} bytes]",
            &text[..end]
        );
    }
    format!(
        "{}\n\n[output truncated: showing {} of {} lines / {} of {} bytes]",
        truncated.content.trim_end(),
        truncated.output_lines,
        truncated.total_lines,
        truncated.output_bytes,
        truncated.total_bytes
    )
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_stay_within_the_provider_limit_and_are_stable() {
        assert_eq!(wire_name("fs", "read_file"), "mcp__fs__read_file");
        // 非安全字符替换掉，不会带出路径或标点。
        assert_ne!(wire_name("a.b", "x/y"), wire_name("a_b", "x_y"));
        assert_ne!(wire_name("a__b", "c"), wire_name("a", "b__c"));
        assert_ne!(wire_name("a_", "b"), wire_name("a", "_b"));
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
    fn huge_text_results_are_truncated_with_a_note() {
        let huge = "x".repeat(pipi_tools::truncate::DEFAULT_MAX_BYTES + 4096);
        let output = render(CallToolResult {
            content: vec![ContentPart::Text(huge)],
            structured: None,
            is_error: false,
        })
        .unwrap();
        let ToolResultContent::Text { text } = &output.content[0] else {
            panic!("expected a text result");
        };
        assert!(text.len() <= pipi_tools::truncate::DEFAULT_MAX_BYTES + 256);
        assert!(
            text.contains("[output truncated:"),
            "missing truncation note"
        );
    }

    #[test]
    fn oversized_structured_details_are_not_persisted() {
        let big = serde_json::json!({ "blob": "y".repeat(MAX_DETAILS_BYTES + 1) });
        let output = render(CallToolResult {
            content: Vec::new(),
            structured: Some(big),
            is_error: false,
        })
        .unwrap();
        // details 不进会话条目；文本形态仍然交给模型（并受同一套截断约束）。
        assert!(output.details.is_none());
        assert!(matches!(&output.content[0], ToolResultContent::Text { .. }));
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
