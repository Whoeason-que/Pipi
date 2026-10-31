//! 内置工具实现。移植自 `packages/agent/src/harness/tools/`，外加 Pipi 新增的
//! memory 工具。
//!
//! 工具**端口**（`AgentTool` / `ToolContext` / `ToolRegistry` / `ToolOutput`）
//! 与权限**数据**（`PermissionsConfig` / `SandboxMode` / 工具名清单）在
//! `pipi_protocol`；这里放实现：内置工具集、输出截断、进程组收尾。真正的
//! 执行边界在 `pipi_sandbox`，装配策略只有 [`builtin_registry`] 一处。

mod bash;
mod edit;
mod file_queue;
pub mod memory;
pub mod process;
mod read;
mod truncate;
mod write;

use std::sync::Arc;

use pipi_protocol::tools::{AgentTool, ToolContext, ToolRegistry};

/// 按 Agent 的权限配置注册内置工具。
pub fn builtin_registry(ctx: &ToolContext) -> ToolRegistry {
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
    if ctx.permissions.tool_enabled("memory")
        && let Some(dir) = &ctx.memory_dir
    {
        tools.push(Arc::new(memory::MemoryTool {
            memory_dir: dir.clone(),
        }));
    }
    ToolRegistry::new(tools)
}
