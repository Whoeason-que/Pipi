//! Agent 环境域：清单与注册表、agent.toml 契约、技能包管理、提示词素材。
//!
//! 「一切皆文件的 Agent 环境」都收在这里 —— 它们共享同一批词汇（Agent 清单、
//! 契约层、SKILL.md、项目上下文文件），拆成多个 crate 只会来回转口：
//!
//! | 模块 | 职责 |
//! | --- | --- |
//! | [`agents`] | Agent 清单（`agent.json`）与 `~/.pipi/agents/` 注册表、系统提示组装 |
//! | [`av`] | `agent.toml` 环境契约（解析 / 发现 / 合并 / env 解析 / 全局技能 store）与 `av` CLI |
//! | [`skills`] | 技能中心：store 浏览、安装、校验与 Agent 启用状态（走同一份 av 实现） |
//! | [`prompt`] | 纯提示词层：项目上下文发现与 system prompt 渲染（不碰运行时与网络） |
//!
//! 依赖面只有契约层（`pipi-protocol`）与内置工具助手（`pipi-tools::memory`）；
//! 宿主（`pipi-core`）在这里读清单、装技能、拿提示词，引擎只收结果。

pub mod agents;
pub mod av;
pub mod prompt;
pub mod skills;
