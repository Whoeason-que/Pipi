//! `pipi-tools` 的 write 工具。移植自 `packages/agent/src/harness/tools/write.ts`：
//! 自动创建父目录，存在则覆盖。

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{AgentTool, ToolContext, ToolOutput, resolve_path};
use crate::permissions::resolve_write_path;
use pipi_protocol::ToolResultContent;

pub struct WriteTool;

#[async_trait]
impl AgentTool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> String {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to write (relative or absolute)" },
                "content": { "type": "string", "description": "Content to write to the file" }
            },
            "required": ["path", "content"]
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
        let path = args["path"].as_str().ok_or("缺少 path")?;
        let content = args["content"].as_str().ok_or("缺少 content")?;

        let abs = if matches!(ctx.sandbox, crate::permissions::SandboxMode::WorkspaceWrite) {
            resolve_write_path(&ctx.workspace, path)?
        } else {
            let abs = resolve_path(&ctx.workspace, path)?;
            ctx.ensure_writable(&abs)?;
            abs
        };
        // 原子替换：覆盖到一半崩溃也不会把原文件毁成半截（父目录由写入助手创建）。
        // 写入是同步的文件系统调用，放到 blocking 线程池，别占住运行时 worker。
        let target = abs.clone();
        let contents = content.to_string();
        tokio::task::spawn_blocking(move || crate::fs::write_atomic(&target, contents))
            .await
            .map_err(|e| format!("Could not write file: {path}. {e}"))?
            .map_err(|e| format!("Could not write file: {path}. {e}"))?;
        if ctx.abort.is_aborted() {
            return Err("Operation aborted".into());
        }

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: format!("Successfully wrote to {path}"),
            }],
            details: None,
            terminate: false,
        })
    }
}

#[cfg(test)]
mod tests;
