//! 内置工具实现与 bash 权限策略。移植自 `packages/agent/src/harness/tools/`，
//! 外加 Pipi 新增的 memory 工具。
//!
//! 工具**端口**（`AgentTool` / `ToolContext` / `ToolRegistry` / `ToolOutput`）
//! 与权限**数据**（`PermissionsConfig` / `SandboxMode` / 工具名清单）在
//! `pipi_protocol`；这里放实现：内置工具集、命令切分与危险命令判定、输出
//! 截断、进程组收尾。装配策略只有 [`builtin_registry`] 一处 —— 宿主在运行
//! 装配时调用它，再按需 `ToolRegistry::push` 注入自己的工具（组合工具、
//! 后台任务工具）。

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod memory;
pub mod permissions;
pub mod process;
pub mod read;
pub mod truncate;
pub mod write;

use std::sync::Arc;

use pipi_protocol::tools::{AgentTool, ToolContext, ToolRegistry};

/// 按 Agent 的权限配置注册内置工具。
pub fn builtin_registry(ctx: &ToolContext) -> ToolRegistry {
    let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
    if ctx.permissions.tool_enabled("read") {
        tools.push(Arc::new(read::ReadTool));
    }
    if ctx.permissions.tool_enabled("glob") {
        tools.push(Arc::new(glob::GlobTool));
    }
    if ctx.permissions.tool_enabled("grep") {
        tools.push(Arc::new(grep::GrepTool));
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
