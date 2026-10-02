//! Agent 组合工具与后台任务托管（基础文件/命令工具见 `pipi-tools`）。
//!
//! 这两个子模块留在 core，因为它们只依赖 core 的 Agent/session port，
//! 具体运行时由 `AgentRunner` 注入。

pub mod agent;
pub mod background;
