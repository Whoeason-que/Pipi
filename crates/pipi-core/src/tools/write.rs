//! write 工具。移植自 `packages/agent/src/harness/tools/write.ts`：
//! 自动创建父目录，存在则覆盖。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resolve_path, AgentTool, ToolContext, ToolOutput};
use crate::types::ToolResultContent;

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

        let abs = resolve_path(&ctx.workspace, path)?;
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Could not create parent directories: {e}"))?;
        }
        tokio::fs::write(&abs, content)
            .await
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
