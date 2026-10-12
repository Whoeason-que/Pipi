//! Pipi 领域层 —— Agent 定义与 av 契约、设置与 Agent 组合工具。
//!
//! 运行时内核（循环、上下文预算、重试、统计、会话存储、压缩）在 `pipi-agent`；
//! 技能中心在 `pipi-skills`（store 浏览 / 安装 / 校验与启用状态）；模型目录在
//! `pipi-provider::catalog`（models.dev 拉取与缓存）。这里保留的是「一切皆文件的 Agent
//! 注册表 + 契约解析 + 设置 + Agent 组合工具」。
//!
//! | 模块 | 职责 | pi / 上游对应 |
//! | --- | --- | --- |
//! | [`agents`] | Agent 定义与注册表、av 契约层解析、system prompt 装配 | （Pipi 新增） |
//! | [`settings`] | `~/.pipi/settings.json`（供应商、压缩、重试策略） | （Pipi 新增） |
//! | [`tools`] | Agent 组合工具（create/read/run agent、后台任务） | `packages/agent` 工具层 |
//!
//! 另有 codex 移植：`pipi_tools::permissions::safety`（危险命令）与
//! `SandboxMode`，见 README「与 codex 的关系」。
//!
//! 尚未移植（有意推迟，需要时再从上游搬运）：
//! hooks 全集、prepareNextTurn、多 provider 目录、图片工具等。

pub mod agents;
pub mod settings;
pub mod tools;
