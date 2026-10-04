//! 前端 DTO（serde camelCase）：技能 store 视图、Agent 启用状态与安装 / 同步
//! 报告。

use av::store::SourceType;
use serde::{Deserialize, Serialize};

/// store 里的一条版本记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillVersion {
    pub content_hash: String,
    pub source: String,
    pub source_type: SourceType,
    pub reference: Option<String>,
    pub revision: Option<String>,
    pub skill_path: Option<String>,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
    /// 内容目录是否存在（廉价检查；哈希漂移由 [`crate::verify_store_skills`] 判定）。
    pub present: bool,
}

/// store 里的一个技能（同名多版本聚合）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreSkill {
    pub name: String,
    /// 当前版本 SKILL.md 的描述（内容缺失时为 null）。
    pub description: Option<String>,
    /// 当前版本（最近安装/更新）的哈希。
    pub current_hash: String,
    pub source: String,
    pub source_type: SourceType,
    pub skill_path: Option<String>,
    pub installed_at: Option<String>,
    /// 全部版本，新的在前。
    pub versions: Vec<SkillVersion>,
}

/// 全局 store 一览。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLibrary {
    pub store_path: String,
    pub skills: Vec<StoreSkill>,
}

/// 已声明技能的解析状态（不解引用哈希；漂移由校验/同步判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSkillStatus {
    /// 锁记录 + store 内容 + SKILL.md 齐备，且未被 only/exclude 过滤。
    Ready,
    /// 有锁记录但 store 内容缺失（同步可修复）。
    Missing,
    /// 内容在但 SKILL.md 缺失或不可解析（会话启动会 fail-closed）。
    Broken,
    /// use 声明了，但同层 agent.lock 里没有解析记录。
    Unresolved,
    /// 解析齐备，但被 only / exclude 过滤，不会注入。
    Filtered,
}

/// `use` 里一个技能及其状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub name: String,
    pub description: Option<String>,
    pub status: AgentSkillStatus,
    pub content_hash: Option<String>,
    pub source: Option<String>,
    pub revision: Option<String>,
    /// 被 only / exclude 过滤时的原因（命中的模式）。
    pub filter: Option<String>,
}

/// store 里可启用（尚未声明）的技能。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableSkill {
    pub name: String,
    pub description: Option<String>,
    pub content_hash: String,
    pub present: bool,
    pub source: String,
    pub source_type: SourceType,
    pub version_count: usize,
}

/// 一个 Agent 的技能启用视图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkills {
    pub agent_name: String,
    /// 启用/禁用会写入的契约文件（最高声明层；没有声明时是 Agent 层的 agent.toml）。
    pub contract_path: String,
    /// 与契约同目录的解析锁 `agent.lock`。
    pub lock_path: String,
    /// 当前生效的 `use` 由谁声明："agent" | "project" | "project-local"；没有声明时为 null。
    pub declared_by: Option<String>,
    pub declared_by_path: Option<String>,
    /// Agent 层 agent.toml 是否已存在（不存在时启用会新建）。
    pub agent_contract_exists: bool,
    /// 声明落在项目层（覆盖 Agent 层）时为 true —— 编辑会写项目文件。
    pub declares_in_project: bool,
    pub declared: Vec<AgentSkill>,
    pub available: Vec<AvailableSkill>,
    pub only: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub sources: Option<Vec<String>>,
    /// 锁不可读（损坏）或声明了 use 却缺锁时的可读提示；不阻塞视图渲染。
    pub lock_error: Option<String>,
}

/// store 内容哈希校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillVerifyStatus {
    Ok,
    Missing,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillVerify {
    pub name: String,
    pub content_hash: String,
    pub status: SkillVerifyStatus,
    /// 漂移时的实际哈希。
    pub actual_hash: Option<String>,
}

/// 一次安装里落进 store 的技能。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillView {
    pub name: String,
    pub content_hash: String,
    pub source: String,
    pub source_type: SourceType,
    pub skill_path: Option<String>,
    pub revision: Option<String>,
    /// store 里已有同内容版本（幂等跳过）。
    pub unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedSkill {
    pub path: String,
    pub reason: String,
}

/// `skill_install` 的结果（含安装日志行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallReport {
    pub installed: Vec<InstalledSkillView>,
    pub skipped: Vec<SkippedSkill>,
    pub log: Vec<String>,
    pub dry_run: bool,
}

/// `sync_agent_skills` 的结果（含同步日志行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSyncReport {
    /// 本来就就绪的。
    pub ready: Vec<String>,
    /// 本次补装内容的。
    pub installed: Vec<String>,
    /// 本次重装漂移内容的。
    pub repaired: Vec<String>,
    /// 本次从 store 索引补上解析锁的。
    pub resolved: Vec<String>,
    pub log: Vec<String>,
}

/// 某技能在各个 Agent 上的启用情况（技能库详情用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUsage {
    pub agent_name: String,
    pub enabled: bool,
    pub status: Option<AgentSkillStatus>,
    pub filter: Option<String>,
    pub declared_by: Option<String>,
}
