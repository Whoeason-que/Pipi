//! Pipi 领域层 —— Agent 定义与 av 契约、技能中心、模型目录与设置。
//!
//! Agent 循环、上下文预算、重试、统计、会话存储与压缩已拆到 `pipi-agent`
//! （可复用的运行时内核，不依赖 av、不做网络目录）；这里保留的是 Pipi 自己的
//! 领域服务：一切皆文件的 Agent 注册表、agent.toml 契约解析、技能中心与模型目录。
//!
//! | 模块 | 职责 | pi / 上游对应 |
//! | --- | --- | --- |
//! | [`agents`] | Agent 定义与注册表、av 契约层解析、system prompt 装配 | （Pipi 新增） |
//! | [`skill_center`] | 技能中心：store 浏览 / 安装 / 校验与 Agent 启用状态 | av 契约 + skills.sh |
//! | [`catalog`] | models.dev 目录拉取与缓存 | opencode 的 models.dev 口径 |
//! | [`settings`] | `~/.pipi/settings.json`（供应商、压缩、重试策略） | （Pipi 新增） |
//! | [`tools`] | Agent 组合工具（create/read/run agent、后台任务） | `packages/agent` 工具层 |
//!
//! 引擎侧模块（[`pipi_agent`]）：`agent_loop`（主循环）、`context`（token 预算）、
//! `compaction`（摘要替换 + 旧工具输出清理）、`session`（树状 JSONL）、`retry`、
//! `stats`。
//!
//! 另有 codex 移植：`pipi_tools::permissions::safety`（危险命令）与
//! `SandboxMode`，见 README「与 codex 的关系」。
//!
//! 尚未移植（有意推迟，需要时再从上游搬运）：
//! hooks 全集、prepareNextTurn、多 provider 目录、图片工具等。

pub mod agents;
pub mod catalog;
pub mod settings;
pub mod skill_center;
pub mod tools;

/// HOME 环境变量是进程级的：所有涉及 `~/.pipi` 的测试（改写 HOME 或依赖
/// 环境 HOME 下的 settings）都必须持有此锁串行执行，否则并行测试互相污染。
#[cfg(test)]
pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 测试内改写进程环境。
///
/// edition 2024 起 `std::env::set_var` 是 unsafe：环境块不是线程安全的。
/// 安全性由调用方保证 —— 先持 [`HOME_LOCK`] 再做环境读写，同一测试二进制内
/// 的改动因此互斥；新增用例照此办理。
#[cfg(test)]
pub(crate) fn set_env_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: 调用方持 HOME_LOCK
    unsafe { std::env::set_var(key, value) };
}

/// 见 [`set_env_var`]：同样要求调用方持 [`HOME_LOCK`]。
#[cfg(test)]
pub(crate) fn remove_env_var(key: &str) {
    // SAFETY: 调用方持 HOME_LOCK
    unsafe { std::env::remove_var(key) };
}
