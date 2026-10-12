//! 技能中心：全局技能 store（`~/.av/skills`）的浏览、安装、校验，以及每个
//! Agent 的 `[resources.skills].use` 启用状态。
//!
//! 与 av 的分工：安全语义（内容哈希、限额、锁语义、fail-closed 判定）全部由
//! av 拥有；这里只做 Pipi 侧的策略与视图组装，写操作与 `av skill
//! install/add/sync/remove` 共用同一份 av 实现（[`crate::av::contract_edit`]、
//! [`crate::av::install`]、[`crate::av::store`]）。
//!
//! 不变量与 av 一致：**安装 ≠ 启用**；**会话启动只读、不联网**（`agents.rs`
//! 的解析路径不经过本模块）；这里的写操作只发生在用户显式点击时。
//!
//! 阻塞说明：[`install_skill`] 与 [`sync_agent_skills`] 可能联网（git 源），
//! 宿主必须放到阻塞线程池执行 —— Tauri 用 `spawn_blocking`，不要占用事件循环。
//!
//! 模块划分：
//! - `model`：前端 DTO（serde camelCase）；
//! - `library`：store 一览、内容校验与技能影响面；
//! - `agent_skills`：单个 Agent 的启用视图与解析锁状态；
//! - `enable`：启用 / 禁用（编辑 `use` 声明 + 维护锁）；
//! - `install`：安装 / 移除与声明同步。

mod agent_skills;
mod enable;
mod install;
mod library;
mod model;

pub use agent_skills::agent_skills;
pub use enable::set_skill_enabled;
pub use install::{install_skill, remove_store_skill, sync_agent_skills};
pub use library::{skill_usage, store_skills, verify_store_skills};
pub use model::{
    AgentSkill, AgentSkillStatus, AgentSkills, AvailableSkill, InstalledSkillView,
    SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify, SkillVerifyStatus,
    SkillVersion, SkippedSkill, StoreSkill,
};
