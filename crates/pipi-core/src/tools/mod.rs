//! core 对工具协议的兼容入口。
//!
//! 基础文件/命令工具和其安全策略属于 `pipi-tools`；Agent 组合工具仍留在 core，
//! 因为它只依赖 core 的 Agent/session port，具体运行时由 `AgentRunner` 注入。

pub mod agent;
pub mod background;

pub use pipi_tools::{
    bash, edit, glob, grep, memory, read, resolve_path, resolve_read_path, resolve_write_path,
    validate_args, write, AgentTool, ToolContext, ToolOutput, ToolRegistry,
};
