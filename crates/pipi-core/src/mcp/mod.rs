//! MCP 服务器接入：把 `agent.json` 的 `mcpServers` 变成会话里可调用的工具。
//!
//! 分工：
//! - [`McpManager`] 持有连接与工具清单（进程级，跨轮次存活）。连接在后台建立，
//!   注册表每轮只读取**此刻已知**的清单，所以 `prepare` 是同步的 —— 它会在持有
//!   会话锁的路径上被调用，不能 await。
//! - [`McpTool`] 是 `AgentTool` 适配器：调用时按需重连，并把 MCP 的 content
//!   映射成 `ToolOutput`。
//! - `cache` 把 `tools/list` 的结果按 (agent, server, 配置指纹) 存到
//!   `<Agent 目录>/mcp-cache/<server>.json`：App 重启后的第一轮就能声明工具，
//!   服务器进程仍然按需拉起。
//!
//! 权限口径：工具开关（`permissions.tools`）只管内置工具；**启用一台 MCP 服务器
//! 就是它那批工具的授权**（`enabled: false` 关闭）。服务器进程跟随 Agent 的沙箱
//! 档位，可用 `mcpServers[].sandbox` 单独覆盖。

mod cache;
mod manager;
mod tool;

pub use manager::{McpManager, McpServerStatus, McpToolSpec};
pub use tool::McpTool;
