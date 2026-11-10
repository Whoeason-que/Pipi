//! `pipi-tools` 的 memory 工具。Agent 的持久记忆就是
//! `~/.pipi/agents/<name>/memory/` 下的 Markdown 文件 —— 人机共写。
//! 工具严格限制在 memory 目录内活动。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use pipi_protocol::permissions::resolve_write_path;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};

pub struct MemoryTool {
    pub memory_dir: PathBuf,
}

fn resolve_in_memory_dir(memory_dir: &Path, path: &str) -> Result<PathBuf, String> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return Err("memory path must be a relative path to memory directory".into());
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("memory path is not allowed to contain ..".into());
    }
    // 与 write/edit 共用同一套 canonical containment：符号链接（含悬空链接）
    // 指向 memory 目录外时一律拒绝，而不是跟随链接读/写。
    resolve_write_path(memory_dir, path)
        .map_err(|_| format!("memory path escapes the memory directory: {path}"))
}

#[async_trait]
impl AgentTool for MemoryTool {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn description(&self) -> String {
        "Read and write your persistent memory (Markdown files, shared with the user). Actions: list (enumerate memory files), read (read one file), write (create or overwrite one file). Use memory to remember user preferences, project background, and lessons across sessions.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "read", "write"], "description": "What to do" },
                "path": { "type": "string", "description": "Relative path within the memory directory (required for read/write)" },
                "content": { "type": "string", "description": "Content to write (required for write)" }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        if ctx.abort.is_aborted() {
            return Err("Operation aborted".into());
        }
        let action = args["action"].as_str().ok_or("Missing action")?;
        match action {
            "list" => {
                let mut names: Vec<String> = Vec::new();
                collect_markdown(&self.memory_dir, 0, &mut names);
                let text = if names.is_empty() {
                    "(memory is empty)".to_string()
                } else {
                    names.join("\n")
                };
                Ok(ToolOutput::text(text))
            }
            "read" => {
                let path = args["path"].as_str().ok_or("Missing path")?;
                let abs = resolve_in_memory_dir(&self.memory_dir, path)?;
                let file = crate::secure_file::SecureFile::open(&self.memory_dir, &abs, false)
                    .map_err(|e| format!("Failed to read memory file {path}: {e}"))?;
                let content = tokio::task::spawn_blocking(move || file.read(16 * 1024 * 1024))
                    .await
                    .map_err(|e| format!("Failed to read memory file {path}: {e}"))?
                    .map_err(|e| format!("Failed to read memory file {path}: {e}"))?;
                Ok(ToolOutput::text(content))
            }
            "write" => {
                let path = args["path"].as_str().ok_or("Missing path")?;
                let content = args["content"].as_str().ok_or("Missing content")?;
                // containment 检查需要 memory 目录本身可 canonicalize。
                tokio::fs::create_dir_all(&self.memory_dir)
                    .await
                    .map_err(|e| format!("Failed to create memory directory: {e}"))?;
                let abs = resolve_in_memory_dir(&self.memory_dir, path)?;
                let _guard = crate::file_queue::lock_cancellable(&abs, &ctx.abort).await?;
                if ctx.abort.is_aborted() {
                    return Err("Operation aborted".into());
                }
                let contents = content.to_string();
                let root = self.memory_dir.clone();
                let abort = ctx.abort.clone();
                tokio::task::spawn_blocking(move || {
                    crate::secure_file::SecureFile::open(&root, &abs, true)?
                        .write(&contents, &abort)
                })
                .await
                .map_err(|e| format!("Failed to write memory file {path}: {e}"))?
                .map_err(|e| format!("Failed to write memory file {path}: {e}"))?;
                Ok(ToolOutput::text(format!("Saved memory to {path}")))
            }
            other => Err(format!("Unknown action: {other}")),
        }
    }
}

/// 递归收集 memory 目录下的 .md 文件（相对路径）。同步实现：memory 目录
/// 很小，不值得为它引入 Box::pin 的递归 future。
/// 供 memory 工具与 agents.rs 的 memory 索引/编辑器列表共用。
pub fn collect_markdown(dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        // 不跟随符号链接：memory 目录里的一条链接不能把遍历带到目录外。
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_markdown(&path, depth + 1, out);
        } else if file_type.is_file() && name.ends_with(".md") {
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string()
                .replace('\\', "/");
            out.push(rel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::AbortSignal;
    use pipi_protocol::ToolResultContent;
    use pipi_protocol::permissions::{PermissionsConfig, SandboxMode};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn context(memory_dir: std::path::PathBuf, workspace: std::path::PathBuf) -> ToolContext {
        ToolContext {
            workspace,
            memory_dir: Some(memory_dir),
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox: SandboxMode::DangerFullAccess,
            sandbox_policy_override: None,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
        }
    }

    async fn call(tool: &MemoryTool, ctx: &ToolContext, args: Value) -> Result<ToolOutput, String> {
        tool.execute(ctx, &args, &|_| {}).await
    }

    #[tokio::test]
    async fn memory_roundtrip_stays_inside_the_directory() {
        let root = pipi_test_support::temp_dir("pipi-memory");
        let memory = root.join("memory");
        let tool = MemoryTool {
            memory_dir: memory.clone(),
        };
        let ctx = context(memory.clone(), root.clone());

        call(
            &tool,
            &ctx,
            json!({"action": "write", "path": "note.md", "content": "hello"}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(memory.join("note.md")).unwrap(),
            "hello"
        );
        let output = call(&tool, &ctx, json!({"action": "read", "path": "note.md"}))
            .await
            .unwrap();
        let ToolResultContent::Text { text } = &output.content[0] else {
            panic!("expected text");
        };
        assert_eq!(text, "hello");

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memory_rejects_symlink_escape_for_read_and_write() {
        use std::os::unix::fs::symlink;

        let root = pipi_test_support::temp_dir("pipi-memory-symlink");
        let memory = root.join("memory");
        std::fs::create_dir_all(&memory).unwrap();
        let outside = root.join("outside.md");
        std::fs::write(&outside, "secret").unwrap();
        symlink(&outside, memory.join("link.md")).unwrap();

        let outside_dir = root.join("outside-dir");
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("secret.md"), "secret").unwrap();
        symlink(&outside_dir, memory.join("linkdir")).unwrap();

        let tool = MemoryTool {
            memory_dir: memory.clone(),
        };
        let ctx = context(memory.clone(), root.clone());

        assert!(
            call(&tool, &ctx, json!({"action": "read", "path": "link.md"}))
                .await
                .is_err(),
            "reading through a symlink must fail"
        );
        assert!(
            call(
                &tool,
                &ctx,
                json!({"action": "read", "path": "linkdir/secret.md"})
            )
            .await
            .is_err(),
            "reading through a directory symlink must fail"
        );
        assert!(
            call(
                &tool,
                &ctx,
                json!({"action": "write", "path": "link.md", "content": "overwrite"})
            )
            .await
            .is_err(),
            "writing through a symlink must fail"
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");

        let mut listed = Vec::new();
        collect_markdown(&memory, 0, &mut listed);
        assert!(
            listed.is_empty(),
            "symlink entries must not be listed: {listed:?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
