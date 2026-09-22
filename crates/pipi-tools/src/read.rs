//! `pipi-tools` 的 read 工具。移植自 `packages/agent/src/harness/tools/read.ts`：
//! 文本按行号截断输出；图片按魔数识别后以 base64 附件返回。

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use super::{resolve_read_path, AgentTool, ToolContext, ToolOutput};
use crate::truncate::{
    count_lines, format_size, truncate_head, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use crate::types::ToolResultContent;

pub struct ReadTool;

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
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
        let total_file_lines = count_lines(&text);
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
mod tests {
    use super::*;

    #[test]
    fn detects_image_magic_bytes() {
        assert_eq!(
            detect_image_mime(&[0x89, b'P', b'N', b'G', 0x0D]),
            Some("image/png")
        );
        assert_eq!(
            detect_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(detect_image_mime(b"GIF89a...."), Some("image/gif"));
        assert_eq!(detect_image_mime(b"hello"), None);
    }

    async fn read_fixture() -> (
        std::path::PathBuf,
        ToolContext,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        use crate::permissions::PermissionsConfig;
        use crate::types::AbortSignal;
        use std::sync::Arc;

        let base = std::env::temp_dir().join(format!(
            "pipi-read-containment-{}",
            crate::session::new_id()
        ));
        let workspace = base.join("workspace");
        let skill_root = base.join("skills");
        let outside = base.join("outside.txt");
        tokio::fs::create_dir_all(&skill_root).await.unwrap();
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::write(workspace.join("inside.txt"), "inside")
            .await
            .unwrap();
        let skill_file = skill_root.join("trusted.md");
        tokio::fs::write(&skill_file, "trusted skill")
            .await
            .unwrap();
        tokio::fs::write(&outside, "outside").await.unwrap();

        let ctx = ToolContext {
            workspace,
            memory_dir: None,
            read_roots: vec![skill_root],
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox: crate::permissions::SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(std::collections::BTreeMap::new()),
            abort: AbortSignal::new(),
            approver: None,
        };
        (base, ctx, skill_file, outside)
    }

    async fn read(ctx: &ToolContext, path: String) -> Result<ToolOutput, String> {
        ReadTool
            .execute(ctx, &json!({ "path": path }), &|_| {})
            .await
    }

    #[tokio::test]
    async fn read_is_contained_by_workspace_and_resolves_canonical_target() {
        let (base, ctx, skill_file, outside) = read_fixture().await;

        let inside = read(&ctx, "inside.txt".into()).await.unwrap();
        assert!(matches!(
            &inside.content[0],
            ToolResultContent::Text { text } if text == "inside"
        ));
        let skill = read(&ctx, skill_file.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(matches!(
            &skill.content[0],
            ToolResultContent::Text { text } if text == "trusted skill"
        ));
        assert!(read(&ctx, outside.to_string_lossy().into_owned())
            .await
            .is_err());
        assert!(read(&ctx, "../outside.txt".into()).await.is_err());

        tokio::fs::remove_dir_all(base).await.unwrap();
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn read_rejects_symlink_to_outside_workspace() {
        let (base, ctx, _skill_file, outside) = read_fixture().await;
        let link = ctx.workspace.join("outside-link.txt");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, &link).unwrap();

        assert!(read(&ctx, "outside-link.txt".into()).await.is_err());

        tokio::fs::remove_dir_all(base).await.unwrap();
    }
}
