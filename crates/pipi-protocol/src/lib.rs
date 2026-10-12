//! Pipi 的契约层：持久化与跨宿主协议类型，以及各层共用的端口定义。
//!
//! 这里的类型同时服务于 session JSONL、Tauri IPC、Web transport 和前端镜像。
//! 不放 provider、工具或载体的**实现**，避免契约层反向牵引运行时。唯一的运行
//! 控制原语是轻量的 [`AbortSignal`]，供 provider、prompt 渲染与工具共同使用。
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`dto`] | 消息 / 模型 / 流事件 / 错误信封（wire 形状） |
//! | [`abort`] | 协作式中止信号 |
//! | [`session`] | 会话条目格式、回放规则与存储端口（[`session::SessionLog`] / [`session::SessionRepo`]） |
//! | [`provider`] | provider 端口：[`provider::Provider`] 与 [`provider::EventStream`] |
//! | [`tools`] | 工具端口：执行结果、执行上下文、[`tools::AgentTool`] 与 [`tools::ToolRegistry`] |
//! | [`permissions`] | 权限与沙箱的公共词汇、[`permissions::PermissionsConfig`] 与写入路径包含检查 |
//! | [`fs`] | 跨层共用的文件系统原语（原子写、符号链接守卫） |
//! | [`compaction`] | 压缩阈值契约（默认值与归一化） |
//! | [`text`] | 渲染进 prompt 的文本助手 |
//! | [`ids`] | 通用 id 与时间戳 |
//!
//! 顶层再导出 DTO 与 [`AbortSignal`]（历史路径 `pipi_protocol::Message` 等保持
//! 有效）；其余模块按属主路径引用。

pub mod abort;
pub mod compaction;
pub mod dto;
pub mod fs;
pub mod ids;
pub mod permissions;
pub mod provider;
pub mod session;
pub mod text;
pub mod tools;

pub use abort::AbortSignal;
pub use dto::*;
pub use ids::{new_id, now_millis};
