//! memory 工具（Pipi 新增）。Agent 的持久记忆就是
//! `~/.pipi/agents/<name>/memory/` 下的 Markdown 文件 —— 人机共写。
//! 工具严格限制在 memory 目录内活动。

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{AgentTool, ToolContext, ToolOutput};

pub struct MemoryTool {
    pub memory_dir: PathBuf,
}

fn resolve_in_memory_dir(memory_dir: &PathBuf, path: &str) -> Result<PathBuf, String> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return Err("memory path 必须是相对 memory 目录的相对路径".into());
    }
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("memory path 不允许包含 ..".into());
    }
    Ok(memory_dir.join(p))
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
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let action = args["action"].as_str().ok_or("缺少 action")?;
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
                let path = args["path"].as_str().ok_or("缺少 path")?;
                let abs = resolve_in_memory_dir(&self.memory_dir, path)?;
                let content = tokio::fs::read_to_string(&abs)
                    .await
                    .map_err(|e| format!("无法读取记忆文件 {path}: {e}"))?;
                Ok(ToolOutput::text(content))
            }
            "write" => {
                let path = args["path"].as_str().ok_or("缺少 path")?;
                let content = args["content"].as_str().ok_or("缺少 content")?;
                let abs = resolve_in_memory_dir(&self.memory_dir, path)?;
                if let Some(parent) = abs.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| format!("无法创建记忆目录: {e}"))?;
                }
                tokio::fs::write(&abs, content)
                    .await
                    .map_err(|e| format!("无法写入记忆文件 {path}: {e}"))?;
                Ok(ToolOutput::text(format!("Saved memory to {path}")))
            }
            other => Err(format!("未知 action: {other}")),
        }
    }
}

/// 递归收集 memory 目录下的 .md 文件（相对路径）。同步实现：memory 目录
/// 很小，不值得为它引入 Box::pin 的递归 future。
fn collect_markdown(dir: &PathBuf, depth: usize, out: &mut Vec<String>) {
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
        if path.is_dir() {
            collect_markdown(&path, depth + 1, out);
        } else if name.ends_with(".md") {
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            out.push(rel);
        }
    }
}
