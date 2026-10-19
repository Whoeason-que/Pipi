//! Pipi 的宿主层 —— 桌面壳与 Web 服务共用的全部业务实现。
//!
//! 「命令表只有一份」：每个命令的 `XxxArgs`（serde camelCase）、实现与
//! [`commands::dispatch`] 都在这里；两个壳（`crates/pipi` 的 Tauri 壳与
//! `--features server` 的 Web 宿主）只做转发。
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`events`] | 宿主事件协议（叶子：approval / runtime 都依赖它） |
//! | [`approval`] | bash 交互审批状态机（fail-closed） |
//! | [`runtime`] | 会话槽、运行编排、压缩重键、会话载体装配点 |
//! | [`background`] | 后台任务托管（shell / child Agent） |
//! | [`commands`] | 宿主命令表（参数结构 + 实现 + dispatch） |
//! | [`settings`] | `~/.pipi/settings.json`（供应商、压缩、重试策略） |
//! | [`tools`] | Agent 组合工具与后台任务工具（基础工具在 `pipi-tools`） |
//! | `server` | Web/远程宿主（`--features server`；只做 transport 与鉴权） |
//!
//! 领域与引擎在别处：Agent 清单 / 注册表 / 技能在 `pipi-venv`，上下文与压缩
//! 策略在 `pipi-context`，会话载体在 `pipi-session`，模型面在 `pipi-provider`。
//!
//! 另有 codex 移植：危险命令判定（`pipi-tools/src/permissions/safety.rs`）与
//! `SandboxMode`，见 README「与 codex 的关系」。
//!
//! 尚未移植（有意推迟，需要时再从上游搬运）：
//! hooks 全集、prepareNextTurn、多 provider 目录、图片工具等。

pub mod approval;
pub mod background;
pub mod commands;
pub mod events;
pub mod forum;
pub mod runtime;
pub mod settings;
pub mod tools;

#[cfg(feature = "server")]
pub mod server;
