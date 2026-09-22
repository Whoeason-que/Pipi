//! `pipi-harness` 的兼容入口。
//!
//! Pipi 核心仍通过此模块保留既有导入路径；纯提示词渲染和项目资源发现已经在
//! 独立 crate 中，因而不会反向依赖 Agent 存储、工具执行或运行时状态。

pub use pipi_harness::*;
