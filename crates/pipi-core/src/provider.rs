//! `pipi-provider` 的兼容入口。
//!
//! 运行时、Agent loop 与外部调用方继续从 `pipi_core::provider` 导入；实际的
//! rig HTTP/SSE 适配已成为独立 crate，不再把传输依赖拉进核心状态层。

pub use pipi_provider::*;
