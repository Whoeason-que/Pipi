//! 上下文与运行策略：token 预算、裁剪与配对修复、摘要压缩流水线、重试退避。
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`context`] | token 估算 / 裁剪 / 环境上下文 / 工具配对修复 |
//! | [`compaction`] | 压缩流水线（投影式清理 + 摘要替换），产出 Plan 交宿主落盘 |
//! | [`retry`] | 可重试错误的退避决策（引擎与压缩摘要共用） |
//!
//! 这里只做策略：不碰会话载体（端口在 `pipi_protocol::session`）、不直连网络
//! （provider 是 `pipi_protocol::provider` 的端口）、不落盘（写会话由宿主的
//! runtime 完成）。

pub mod compaction;
pub mod context;
pub mod retry;

#[cfg(test)]
mod test_support;
