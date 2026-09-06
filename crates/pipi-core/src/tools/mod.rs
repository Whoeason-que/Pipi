//! 内置工具集。移植自 `packages/agent/src/harness/tools/`，外加 Pipi 新增的
//! memory 工具。工具按 `permissions.tools` 注册；`ToolContext` 携带工作目录、
//! 权限与中止信号。

pub mod bash;
pub mod edit;
pub mod memory;
pub mod read;
pub mod write;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::permissions::{PermissionsConfig, SandboxMode};
use crate::types::{AbortSignal, Tool, ToolResultContent};

/// 工具执行结果。对应 pi 的 `AgentToolResult`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub content: Vec<ToolResultContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminate: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            content: vec![ToolResultContent::Text { text: text.into() }],
            details: None,
            terminate: false,
        }
    }
}

/// 工具执行上下文：工作目录、memory 目录、权限、沙箱、中止信号。
#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub memory_dir: Option<PathBuf>,
    pub permissions: Arc<PermissionsConfig>,
    pub sandbox: SandboxMode,
    pub abort: AbortSignal,
}

impl ToolContext {
    /// 沙箱下的写入约束（移植自 codex 的 SandboxMode 语义）：
    /// - `danger-full-access`：不限制
    /// - `workspace-write`：只能写工作目录内
    /// - `read-only`：禁止写文件（memory 除外 —— 那是 Agent 自己的脑子）
    pub fn ensure_writable(&self, path: &Path) -> Result<(), String> {
        match self.sandbox {
            SandboxMode::DangerFullAccess => Ok(()),
            SandboxMode::ReadOnly => Err("沙箱策略为 read-only：禁止写入文件".into()),
            SandboxMode::WorkspaceWrite => {
                if crate::permissions::is_within(&self.workspace, path) {
                    Ok(())
                } else {
                    Err(format!(
                        "沙箱策略为 workspace-write：{} 在工作目录 {} 之外",
                        path.display(),
                        self.workspace.display()
                    ))
                }
            }
        }
    }
}

/// 工具 trait。对应 pi 的 `AgentTool`（typebox schema 换成 JSON Schema）。
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> String;
    fn parameters(&self) -> Value;
    /// 有副作用、不适合与其他调用并发的工具（如 bash）返回 true。
    fn requires_sequential(&self) -> bool {
        false
    }
    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String>;
}

/// 把相对路径解析到工作目录下（绝对路径原样通过）。同 pi 的 path-utils。
pub fn resolve_path(workspace: &Path, path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        Ok(workspace.join(p))
    }
}

/// 按工具名查 schema 里的 required 字段做轻量校验（完整 JSON Schema 校验
/// 是后续项，见 lib.rs「尚未移植」）。
pub fn validate_args(parameters: &Value, args: &Value) -> Result<(), String> {
    if !args.is_object() {
        return Err("工具参数必须是 JSON 对象".into());
    }
    if let Some(required) = parameters["required"].as_array() {
        for name in required {
            if let Some(name) = name.as_str() {
                if args.get(name).is_none() {
                    return Err(format!("缺少必需参数: {name}"));
                }
            }
        }
    }
    Ok(())
}

pub struct ToolRegistry {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn AgentTool>>) -> ToolRegistry {
        ToolRegistry { tools }
    }

    /// 按 Agent 的权限配置注册内置工具。
    pub fn for_context(ctx: &ToolContext) -> ToolRegistry {
        let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        if ctx.permissions.tool_enabled("read") {
            tools.push(Arc::new(read::ReadTool));
        }
        if ctx.permissions.tool_enabled("write") {
            tools.push(Arc::new(write::WriteTool));
        }
        if ctx.permissions.tool_enabled("edit") {
            tools.push(Arc::new(edit::EditTool));
        }
        if ctx.permissions.tool_enabled("bash") {
            tools.push(Arc::new(bash::BashTool));
        }
        if ctx.permissions.tool_enabled("memory") {
            if let Some(dir) = &ctx.memory_dir {
                tools.push(Arc::new(memory::MemoryTool {
                    memory_dir: dir.clone(),
                }));
            }
        }
        ToolRegistry { tools }
    }

    pub fn wire_tools(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(|t| Tool {
                name: t.name().to_string(),
                description: t.description(),
                parameters: t.parameters(),
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}
