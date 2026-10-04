//! `pipi-tools` 的 read 工具。移植自 `packages/agent/src/harness/tools/read.ts`：
//! 文本按行号截断输出；图片按魔数识别后以 base64 附件返回。

use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};

use super::{AgentTool, ToolContext, ToolOutput, resolve_read_path};
use crate::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, format_size, truncate_head};
use crate::types::ToolResultContent;

pub struct ReadTool;

/// 图片识别交给 `infer`（同一批魔数，但覆盖面更全）。
/// BMP 仍然只识别不转码：转码需要完整的图像解码器，当前不值这个依赖。
/// 返回 (mime, 是否可直接作为附件发送)。
fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    match infer::get(bytes)?.mime_type() {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "image/bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[async_trait]
impl AgentTool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> String {
        format!(
            "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
            DEFAULT_MAX_BYTES / 1024
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
                "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
                "limit": { "type": "number", "description": "Maximum number of lines to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let path = args["path"].as_str().ok_or("缺少 path")?;
        let offset = args["offset"].as_u64().map(|v| v as usize);
        let limit = args["limit"].as_u64().map(|v| v as usize);

        let abs = resolve_read_path(&ctx.workspace, &ctx.read_roots, path)?;
        let bytes = tokio::fs::read(&abs)
            .await
            .map_err(|e| format!("Could not read file: {path}. {e}"))?;

        if let Some(mime) = detect_image_mime(&bytes) {
            if mime == "image/bmp" {
                return Ok(ToolOutput::text(
                    "Read image file [image/bmp]\n[Image omitted: BMP conversion is not built in.]",
                ));
            }
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(ToolOutput {
                content: vec![
                    ToolResultContent::Text {
                        text: format!("Read image file [{mime}]"),
                    },
                    ToolResultContent::Image {
                        data,
                        mime_type: mime.to_string(),
                    },
                ],
                details: None,
                terminate: false,
            });
        }

        let text = String::from_utf8_lossy(&bytes);
        let all_lines: Vec<&str> = text.split('\n').collect();
        // 行数只有这一个口径（与 pi 一致）：越界判定、剩余行数、提示文案都用它。
        // 另算一份 count_lines 会让「of N lines」与边界判断互相矛盾
        //（文件以换行结尾时两者差 1）。
        let total_file_lines = all_lines.len();
        let start_line = offset.map(|o| o.saturating_sub(1)).unwrap_or(0);
        let start_line_display = start_line + 1;
        if start_line >= all_lines.len() {
            return Err(format!(
                "Offset {} is beyond end of file ({} lines total)",
                offset.unwrap_or(1),
                all_lines.len()
            ));
        }

        let (selected_content, user_limited_lines): (String, Option<usize>) = match limit {
            Some(limit) => {
                let end_line = std::cmp::min(start_line + limit, all_lines.len());
                (
                    all_lines[start_line..end_line].join("\n"),
                    Some(end_line - start_line),
                )
            }
            None => (all_lines[start_line..].join("\n"), None),
        };

        let truncation = truncate_head(&selected_content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        let output_text;
        let mut details: Option<Value> = None;

        if truncation.first_line_exceeds_limit {
            let first_line_size = format_size(all_lines[start_line].len());
            output_text = format!(
                "[Line {start_line_display} is {first_line_size}, exceeds {} limit. Use bash: sed -n '{start_line_display}p' {path} | head -c {DEFAULT_MAX_BYTES}]",
                format_size(DEFAULT_MAX_BYTES)
            );
            details = Some(json!({ "truncation": { "truncated": true } }));
        } else if truncation.truncated {
            let end_line_display = start_line_display + truncation.output_lines - 1;
            let next_offset = end_line_display + 1;
            let mut text = truncation.content.clone();
            if truncation.truncated_by == Some("lines") {
                text.push_str(&format!(
                    "\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines}. Use offset={next_offset} to continue.]"
                ));
            } else {
                text.push_str(&format!(
                    "\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines} ({} limit). Use offset={next_offset} to continue.]",
                    format_size(DEFAULT_MAX_BYTES)
                ));
            }
            output_text = text;
            details = Some(json!({ "truncation": { "truncated": true } }));
        } else if let Some(user_limited) = user_limited_lines {
            if start_line + user_limited < all_lines.len() {
                let remaining = all_lines.len() - (start_line + user_limited);
                let next_offset = start_line + user_limited + 1;
                output_text = format!(
                    "{}\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]",
                    truncation.content
                );
            } else {
                output_text = truncation.content.clone();
            }
        } else {
            output_text = truncation.content.clone();
        }

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text { text: output_text }],
            details,
            terminate: false,
        })
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
