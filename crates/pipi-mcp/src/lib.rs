//! MCP（Model Context Protocol）的 stdio 客户端。
//!
//! 只做协议与进程管理，不认识 Pipi 的 Agent、权限或工具 DTO：调用方（宿主
//! `pipi-core`）负责按 Agent 定义装配 [`Command`]（沙箱、env、工作目录），并把
//! [`McpToolDef`] 适配成自己的工具实现。
//!
//! 传输按 MCP stdio 规范：**一行一条 JSON-RPC 消息**（不是 LSP 的
//! `Content-Length` 头）。请求按 id 多路复用，读任务独立于调用方，因此同一
//! 连接上的多个工具调用可以并发；写入侧按消息串行，避免两条消息交错。
//!
//! 生命周期：`connect` 负责 spawn + `initialize` 握手；`shutdown` 先关 stdin
//! （正常退出的约定），再整组 `SIGTERM`，最后 `SIGKILL` —— 只杀直接子进程会
//! 留下 `npx` / `uvx` 的孙进程继续持有管道。

mod client;
mod error;
mod protocol;

pub use client::{ConnectOptions, McpClient};
pub use error::McpError;
pub use protocol::{CallToolResult, ContentPart, McpToolDef, ServerInfo};
pub use tokio::process::Command;
