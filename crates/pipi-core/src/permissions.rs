//! `pipi-tools` 的权限策略兼容入口。
//!
//! 安全判定与 shell 解析已随工具实现下沉；core 仅消费稳定的权限配置和判定
//! 结果，保留旧模块路径以避免一次性破坏调用方。

pub use pipi_tools::permissions::*;
