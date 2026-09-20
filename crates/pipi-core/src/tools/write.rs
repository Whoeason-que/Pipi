//! write 工具。移植自 `packages/agent/src/harness/tools/write.ts`：
//! 自动创建父目录，存在则覆盖。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resolve_path, resolve_write_path, AgentTool, ToolContext, ToolOutput};
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

        let abs = if matches!(ctx.sandbox, crate::permissions::SandboxMode::WorkspaceWrite) {
            resolve_write_path(&ctx.workspace, path)?
        } else {
            let abs = resolve_path(&ctx.workspace, path)?;
            ctx.ensure_writable(&abs)?;
            abs
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{PermissionsConfig, SandboxMode};
    use crate::types::AbortSignal;
    use std::sync::Arc;

    fn ctx(workspace: std::path::PathBuf, sandbox: SandboxMode) -> ToolContext {
        ToolContext {
            workspace,
            memory_dir: None,
            read_roots: Vec::new(),
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox,
            resolved_env: Arc::new(std::collections::BTreeMap::new()),
            abort: AbortSignal::new(),
            approver: None,
        }
    }

    #[tokio::test]
    async fn workspace_write_blocks_outside_paths() {
        let ws = std::env::temp_dir().join(format!("pipi-ws-{}", crate::session::new_id()));
        tokio::fs::create_dir_all(&ws).await.unwrap();
        let ctx = ctx(ws.clone(), SandboxMode::WorkspaceWrite);

        // 工作目录内：放行
        let out = WriteTool
            .execute(&ctx, &json!({"path": "a.txt", "content": "hi"}), &|_| {})
            .await;
        assert!(out.is_ok());

        // 工作目录外：拒绝
        let outside =
            std::env::temp_dir().join(format!("pipi-outside-{}.txt", crate::session::new_id()));
        let err = WriteTool
            .execute(
                &ctx,
                &json!({"path": outside.to_string_lossy(), "content": "hi"}),
                &|_| {},
            )
            .await;
        assert!(err.is_err());

        // ../ 逃逸：拒绝
        let err = WriteTool
            .execute(
                &ctx,
                &json!({"path": "../escape.txt", "content": "hi"}),
                &|_| {},
            )
            .await;
        assert!(err.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workspace_write_blocks_symlinked_parent_outside_workspace() {
        let root =
            std::env::temp_dir().join(format!("pipi-write-link-{}", crate::session::new_id()));
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();

        let ctx = ctx(workspace.clone(), SandboxMode::WorkspaceWrite);
        let result = WriteTool
            .execute(
                &ctx,
                &json!({"path": "link/created.txt", "content": "blocked"}),
                &|_| {},
            )
            .await;

        assert!(result.is_err());
        assert!(!outside.join("created.txt").exists());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workspace_write_blocks_dangling_symlink_to_outside() {
        let root =
            std::env::temp_dir().join(format!("pipi-write-dangling-{}", crate::session::new_id()));
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        let outside_target = outside.join("created.txt");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        std::os::unix::fs::symlink(&outside_target, workspace.join("link.txt")).unwrap();

        let ctx = ctx(workspace, SandboxMode::WorkspaceWrite);
        let result = WriteTool
            .execute(
                &ctx,
                &json!({"path": "link.txt", "content": "blocked"}),
                &|_| {},
            )
            .await;
        let outside_exists = outside_target.exists();
        let _ = tokio::fs::remove_dir_all(root).await;

        assert!(result.is_err());
        assert!(!outside_exists);
    }

    #[tokio::test]
    async fn read_only_blocks_writes_but_full_access_allows() {
        let ws = std::env::temp_dir().join(format!("pipi-ws-{}", crate::session::new_id()));
        tokio::fs::create_dir_all(&ws).await.unwrap();

        let ro = ctx(ws.clone(), SandboxMode::ReadOnly);
        let err = WriteTool
            .execute(&ro, &json!({"path": "a.txt", "content": "hi"}), &|_| {})
            .await;
        assert!(err.is_err());

        let full = ctx(ws, SandboxMode::DangerFullAccess);
        let out = WriteTool
            .execute(&full, &json!({"path": "a.txt", "content": "hi"}), &|_| {})
            .await;
        assert!(out.is_ok());
    }
}
