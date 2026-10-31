//! `pipi-tools` 的 notebook 工具：当前会话的工作文档。
//!
//! notebook 是一个 Markdown 文件（`<Agent 目录>/notebooks/<session>.md`），由
//! Agent 维护、在右侧检查器里渲染给用户看。它不是模型的草稿纸，而是
//! 「计划 / 进度 / 走查」合一的可读交付物：会话重开、压缩之后仍然在。
//! 路径由宿主给定（`ToolContext::notebook_path`），工具只在这一个文件上活动。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use pipi_protocol::fs::{ensure_real_directory, ensure_real_file, write_atomic};
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};

/// notebook 的硬上限。它是给人读的工作文档，不是日志或命令输出转录；
/// 超过就拒绝写入，让 Agent 自己摘要，而不是把整段输出倒进来。
pub const MAX_NOTEBOOK_BYTES: usize = 64 * 1024;

pub struct NotebookTool {
    /// 宿主推出的会话 notebook 路径（`AgentDefinition::notebook_path`）。
    pub path: PathBuf,
}

/// 读 notebook；不存在等于空（第一次写之前是常态，不是错误）。
async fn read_notebook(path: &Path) -> Result<String, String> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("Failed to read notebook: {error}")),
    }
}

async fn write_notebook(path: &Path, content: &str) -> Result<(), String> {
    if content.len() > MAX_NOTEBOOK_BYTES {
        return Err(format!(
            "notebook is limited to {} KiB; keep it a summary (plan, decisions, walkthrough), \
             not raw output",
            MAX_NOTEBOOK_BYTES / 1024
        ));
    }
    // notebook 的位置由宿主给死：目录或文件被换成符号链接时拒绝写入，
    // 否则一条链接就能把内容写到 Agent 目录之外。
    let target = path.to_path_buf();
    let contents = content.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create notebook directory: {e}"))?;
            ensure_real_directory(parent, "notebook directory")?;
        }
        ensure_real_file(&target, "notebook")?;
        write_atomic(&target, contents).map_err(|e| format!("Failed to write notebook: {e}"))
    })
    .await
    .map_err(|e| format!("Failed to write notebook: {e}"))?
}

/// 人工改大的 notebook 也要有界：按字符边界截断并补充说明。
fn bounded(text: String) -> String {
    if text.len() <= MAX_NOTEBOOK_BYTES {
        return text;
    }
    let mut end = MAX_NOTEBOOK_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[notebook truncated at {} KiB]",
        &text[..end],
        MAX_NOTEBOOK_BYTES / 1024
    )
}

#[async_trait]
impl AgentTool for NotebookTool {
    fn name(&self) -> &'static str {
        "notebook"
    }

    fn description(&self) -> String {
        "Read and update this session's notebook: a Markdown working document the user sees in \
         the right sidebar. Keep `## Plan` as a `- [ ]` checklist, record key decisions and \
         findings under `## Notes`, and end with `## Walkthrough` (what changed and how it was \
         verified). Read it before resuming work."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "append"],
                    "description": "read the notebook, replace it (write), or append a section (append)"
                },
                "content": {
                    "type": "string",
                    "description": "Markdown content (required for write / append)"
                }
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
        // 同一批里的 write / append 是并行执行的：读改写整段必须串行。
        let _file_lock = crate::file_queue::lock(&self.path).await;
        match action {
            "read" => {
                let text = bounded(read_notebook(&self.path).await?);
                Ok(ToolOutput::text(if text.is_empty() {
                    "(notebook is empty)".to_string()
                } else {
                    text
                }))
            }
            "write" => {
                let content = args["content"].as_str().ok_or("Missing content")?;
                write_notebook(&self.path, content).await?;
                Ok(ToolOutput::text("Notebook updated"))
            }
            "append" => {
                let content = args["content"].as_str().ok_or("Missing content")?;
                let mut next = read_notebook(&self.path).await?;
                if !next.is_empty() {
                    if !next.ends_with('\n') {
                        next.push('\n');
                    }
                    next.push('\n');
                }
                next.push_str(content);
                write_notebook(&self.path, &next).await?;
                Ok(ToolOutput::text("Notebook updated"))
            }
            other => Err(format!("Unknown action: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::AbortSignal;
    use pipi_protocol::permissions::{PermissionsConfig, SandboxMode};
    use std::sync::Arc;

    fn context() -> ToolContext {
        ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox: SandboxMode::WorkspaceWrite,
            resolved_env: Arc::new(std::collections::BTreeMap::new()),
            abort: AbortSignal::new(),
        }
    }

    fn tool() -> (NotebookTool, std::path::PathBuf) {
        let dir = pipi_test_support::temp_dir("pipi-notebook");
        let path = dir.join("notebooks").join("1730000000000-abc.md");
        (NotebookTool { path: path.clone() }, dir)
    }

    async fn run(tool: &NotebookTool, args: Value) -> Result<ToolOutput, String> {
        tool.execute(&context(), &args, &|_| {}).await
    }

    fn text(output: &ToolOutput) -> String {
        output
            .content
            .iter()
            .map(|part| match part {
                pipi_protocol::ToolResultContent::Text { text } => text.clone(),
                _ => String::new(),
            })
            .collect()
    }

    #[tokio::test]
    async fn read_before_first_write_is_empty_not_an_error() {
        let (tool, dir) = tool();
        let out = run(&tool, json!({"action": "read"})).await.unwrap();
        assert_eq!(text(&out), "(notebook is empty)");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn write_then_read_roundtrip_creates_parent_directories() {
        let (tool, dir) = tool();
        let body = "## Plan\n\n- [ ] inspect\n- [ ] fix\n";
        run(&tool, json!({"action": "write", "content": body}))
            .await
            .unwrap();
        let out = run(&tool, json!({"action": "read"})).await.unwrap();
        assert_eq!(text(&out), body);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn append_adds_a_section_without_losing_existing_content() {
        let (tool, dir) = tool();
        run(
            &tool,
            json!({"action": "write", "content": "## Plan\n\n- [ ] inspect"}),
        )
        .await
        .unwrap();
        run(
            &tool,
            json!({"action": "append", "content": "## Notes\n\nfound it"}),
        )
        .await
        .unwrap();
        let out = run(&tool, json!({"action": "read"})).await.unwrap();
        assert_eq!(
            text(&out),
            "## Plan\n\n- [ ] inspect\n\n## Notes\n\nfound it"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn oversize_writes_are_rejected_and_leave_the_file_untouched() {
        let (tool, dir) = tool();
        run(&tool, json!({"action": "write", "content": "kept"}))
            .await
            .unwrap();
        let too_big = "x".repeat(MAX_NOTEBOOK_BYTES + 1);
        let err = run(&tool, json!({"action": "write", "content": too_big}))
            .await
            .unwrap_err();
        assert!(err.contains("limited to"), "{err}");
        // append 溢出同样拒绝，且不会破坏已有内容。
        let err = run(&tool, json!({"action": "append", "content": too_big}))
            .await
            .unwrap_err();
        assert!(err.contains("limited to"), "{err}");
        let out = run(&tool, json!({"action": "read"})).await.unwrap();
        assert_eq!(text(&out), "kept");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn unknown_action_is_an_error() {
        let (tool, dir) = tool();
        let err = run(&tool, json!({"action": "delete"})).await.unwrap_err();
        assert!(err.contains("Unknown action"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_notebook_file_is_refused() {
        let (tool, dir) = tool();
        std::fs::create_dir_all(tool.path.parent().unwrap()).unwrap();
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "original").unwrap();
        std::os::unix::fs::symlink(&outside, &tool.path).unwrap();

        let err = run(&tool, json!({"action": "write", "content": "overwritten"}))
            .await
            .unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "original");
        std::fs::remove_dir_all(dir).ok();
    }
}
