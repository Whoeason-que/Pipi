//! `pipi-tools` 的 read 工具。移植自 `packages/agent/src/harness/tools/read.ts`：
//! 文本按行号截断输出；图片按魔数识别后以 base64 附件返回；目录按字母序列出
//! 条目（含点文件，子目录带 `/` 后缀）。

use std::io::Read;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};

use crate::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, format_size, truncate_head};
use pipi_protocol::ToolResultContent;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput, resolve_read_path};

pub struct ReadTool;

/// 目录一次返回的条目上限（与 pi 的 `ls` 一致）；用 offset/limit 继续翻。
const DEFAULT_DIR_ENTRIES: usize = 500;
const MAX_DIR_ENTRIES: usize = 2000;

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
            "Read a text file, view an image (jpg, png, gif, webp, bmp), or list a directory. Images are sent as attachments. Text output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Directory entries are sorted alphabetically, include dotfiles, and use a '/' suffix for subdirectories. Use offset/limit to page through files or directories. Files larger than 16 MiB and images larger than 5 MiB are rejected; use bash to read a bounded range.",
            DEFAULT_MAX_BYTES / 1024
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file or directory to read (relative or absolute)" },
                "offset": { "type": "number", "description": "Line number (1-indexed) for files, or the first directory entry to return" },
                "limit": { "type": "number", "description": "Maximum lines for files, or entries for directories (default 500 for directories)" }
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
        let path = args["path"].as_str().ok_or("Missing path")?;
        let offset = args["offset"].as_u64().map(|v| v as usize);
        let limit = args["limit"].as_u64().map(|v| v as usize);

        let abs = resolve_read_path(&ctx.workspace, &ctx.read_roots, path)?;
        // Open through trusted directory handles. A path validated above may
        // have been replaced by a symlink before the actual read.
        let root = std::iter::once(&ctx.workspace)
            .chain(ctx.read_roots.iter())
            .filter_map(|root| std::fs::canonicalize(root).ok())
            .find(|root| abs.starts_with(root))
            .ok_or("No trusted root for read")?;
        let file = crate::secure_file::open_read(&root, &abs)
            .map_err(|e| format!("Could not read path: {path}. {e}"))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("Could not read path: {path}. {e}"))?;
        if metadata.is_dir() {
            return read_directory(path, file, offset, limit).await;
        }
        if !metadata.is_file() {
            return Err(format!(
                "Could not read file: {path}. Path is not a regular file."
            ));
        }
        const MAX_READ_BYTES: usize = 16 * 1024 * 1024;
        let bytes = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            file.take(MAX_READ_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > MAX_READ_BYTES {
                return Err(format!(
                    "File exceeds {MAX_READ_BYTES} bytes; use bash to read a bounded range"
                ));
            }
            Ok(bytes)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e: String| format!("Could not read file: {path}. {e}"))?;

        if let Some(mime) = detect_image_mime(&bytes) {
            if bytes.len() > 5 * 1024 * 1024 {
                return Err("Image exceeds the 5 MiB limit".into());
            }
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

/// 列目录：名称按大小写不敏感排序，子目录带 `/`，点文件一并列出（这是模型
/// 看 `.github` / `.agents` / `.env` 的唯一入口）。`offset`/`limit` 按条目翻页。
async fn read_directory(
    display_path: &str,
    file: std::fs::File,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ToolOutput, String> {
    #[cfg(unix)]
    let mut names: Vec<(String, bool)> =
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let mut dir = rustix::fs::Dir::read_from(&file).map_err(|e| e.to_string())?;
            let mut names = Vec::new();
            while let Some(entry) = dir.read() {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "." || name == ".." {
                    continue;
                }
                if names.len() >= 100_000 {
                    return Err("Directory exceeds 100000 entries".into());
                }
                names.push((name, entry.file_type() == rustix::fs::FileType::Directory));
            }
            Ok(names)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Could not read directory: {display_path}. {e}"))?;
    #[cfg(not(unix))]
    let mut names: Vec<(String, bool)> = {
        let _ = file;
        return Err("Secure directory reads are unavailable on this platform".into());
    };
    names.sort_by(|left, right| {
        left.0
            .to_lowercase()
            .cmp(&right.0.to_lowercase())
            .then_with(|| left.0.cmp(&right.0))
    });

    let total = names.len();
    let start = offset.unwrap_or(1).saturating_sub(1);
    if total == 0 {
        if start == 0 {
            return Ok(ToolOutput::text(format!(
                "Read directory {display_path}: (empty directory)"
            )));
        }
        return Err(format!(
            "Offset {} is beyond end of directory (0 entries total)",
            offset.unwrap_or(1)
        ));
    }
    if start >= total {
        return Err(format!(
            "Offset {} is beyond end of directory ({total} entries total)",
            offset.unwrap_or(1)
        ));
    }
    let take = limit
        .unwrap_or(DEFAULT_DIR_ENTRIES)
        .clamp(1, MAX_DIR_ENTRIES);

    // 只给真正要返回的条目做 stat（判断目录后缀），大目录不做全量 stat。
    let mut selected: Vec<String> = Vec::with_capacity(take.min(total - start));
    for (name, is_dir) in names.into_iter().skip(start).take(take) {
        selected.push(if is_dir { format!("{name}/") } else { name });
    }

    let truncation = truncate_head(&selected.join("\n"), DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    let shown = if truncation.truncated {
        truncation.output_lines
    } else {
        selected.len()
    };
    let end = start + shown;
    let mut text = format!(
        "Read directory {display_path} (entries {}-{end} of {total}):\n{}",
        start + 1,
        truncation.content
    );
    if end < total {
        text.push_str(&format!(
            "\n\n[{} more entries. Use offset={} to continue.]",
            total - end,
            end + 1
        ));
    }
    Ok(ToolOutput::text(text))
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
        // RIFF....WEBP：infer 需要完整的容器头
        assert_eq!(
            detect_image_mime(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            detect_image_mime(b"BM\x36\x00\x00\x00\x00\x00"),
            Some("image/bmp")
        );
        // 不在附件白名单内的格式（如 SVG 文本、TIFF）不当作图片
        assert_eq!(
            detect_image_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            None
        );
        assert_eq!(detect_image_mime(b"II*\x00\x08\x00\x00\x00"), None);
        assert_eq!(detect_image_mime(b"hello"), None);
    }

    async fn read_fixture() -> (
        std::path::PathBuf,
        ToolContext,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        use pipi_protocol::AbortSignal;
        use pipi_protocol::permissions::PermissionsConfig;
        use std::sync::Arc;

        let base = std::env::temp_dir().join(format!(
            "pipi-read-containment-{}",
            pipi_test_support::new_id()
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
            notebook_path: None,
            read_roots: vec![skill_root],
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(std::collections::BTreeMap::new()),
            abort: AbortSignal::new(),
        };
        (base, ctx, skill_file, outside)
    }

    async fn read(ctx: &ToolContext, path: String) -> Result<ToolOutput, String> {
        ReadTool
            .execute(ctx, &json!({ "path": path }), &|_| {})
            .await
    }

    #[tokio::test]
    async fn rejects_large_files_before_unbounded_allocation() {
        let (base, ctx, _, _) = read_fixture().await;
        let path = ctx.workspace.join("large.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        let result = read(&ctx, "large.txt".into()).await;
        assert!(result.unwrap_err().contains("exceeds"));
        tokio::fs::remove_dir_all(base).await.unwrap();
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
        assert!(
            read(&ctx, outside.to_string_lossy().into_owned())
                .await
                .is_err()
        );
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

    fn text_of(output: &ToolOutput) -> String {
        match &output.content[0] {
            ToolResultContent::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        }
    }

    #[tokio::test]
    async fn reads_directories_with_dir_suffix_and_dotfiles() {
        let (base, ctx, _skill_file, _outside) = read_fixture().await;
        let dir = ctx.workspace.join("src");
        tokio::fs::create_dir_all(dir.join("nested")).await.unwrap();
        for name in ["b.rs", "A.rs", ".env"] {
            tokio::fs::write(dir.join(name), "").await.unwrap();
        }

        let text = text_of(&read(&ctx, "src".into()).await.unwrap());
        assert!(
            text.contains("Read directory src (entries 1-4 of 4):"),
            "{text}"
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], ".env");
        assert_eq!(lines[2], "A.rs");
        assert_eq!(lines[3], "b.rs");
        assert_eq!(lines[4], "nested/");

        tokio::fs::remove_dir_all(base).await.unwrap();
    }

    #[tokio::test]
    async fn directory_offset_and_limit_page_entries() {
        let (base, ctx, _skill_file, _outside) = read_fixture().await;
        let dir = ctx.workspace.join("src");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
            tokio::fs::write(dir.join(name), "").await.unwrap();
        }

        let output = ReadTool
            .execute(
                &ctx,
                &json!({ "path": "src", "offset": 2, "limit": 2 }),
                &|_| {},
            )
            .await
            .unwrap();
        let text = text_of(&output);
        assert!(text.contains("entries 2-3 of 5"), "{text}");
        assert!(text.contains("Use offset=4 to continue"), "{text}");

        tokio::fs::remove_dir_all(base).await.unwrap();
    }

    #[tokio::test]
    async fn empty_directory_and_offset_past_end() {
        let (base, ctx, _skill_file, _outside) = read_fixture().await;
        let dir = ctx.workspace.join("empty");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(ctx.workspace.join("only.txt"), "x")
            .await
            .unwrap();

        let text = text_of(&read(&ctx, "empty".into()).await.unwrap());
        assert_eq!(text, "Read directory empty: (empty directory)");

        let beyond = ReadTool
            .execute(&ctx, &json!({ "path": "empty", "offset": 2 }), &|_| {})
            .await;
        assert!(beyond.is_err());

        tokio::fs::remove_dir_all(base).await.unwrap();
    }
}
