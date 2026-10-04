//! Tauri 命令层 —— 只是把 pipi-core / pipi-app 的能力暴露给前端，不含业务逻辑。

use pipi_app::runtime::RuntimeState;
use pipi_core::agents::{self, AgentDefinition};
use pipi_core::catalog::ModelCatalog;
use pipi_core::settings::{self, Settings};
use pipi_core::skill_center::{
    self, AgentSkills, SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify,
};
use pipi_protocol::Model;
use pipi_tools::permissions::PermissionsConfig;
use tauri::State;

#[tauri::command]
pub fn list_agents() -> Result<Vec<AgentDefinition>, String> {
    // 首次启动时播种默认 Agent（Pipi），已存在则原样返回
    agents::list_agents_bootstrapped()
}

/// 模型目录（models.dev + 本地缓存）。必须 async：它要联网，Tauri 会把它放到
/// 自己的异步运行时上执行，从而拿到 IO 驱动（同步命令跑在 GTK 主线程，那里没有）。
#[tauri::command]
pub async fn model_catalog(refresh: Option<bool>) -> Result<ModelCatalog, String> {
    pipi_core::catalog::load_catalog(refresh.unwrap_or(false)).await
}

#[tauri::command]
pub fn create_agent(
    name: String,
    description: String,
    workspace: Option<String>,
    permissions: Option<PermissionsConfig>,
    model: Option<String>,
    provider: Option<Model>,
) -> Result<AgentDefinition, String> {
    agents::create_agent(
        &name,
        &description,
        workspace.as_deref(),
        permissions,
        model.as_deref(),
        provider,
    )
}

#[tauri::command]
pub fn load_agent(name: String) -> Result<AgentDefinition, String> {
    agents::load_agent(&name)
}

#[tauri::command]
pub fn save_agent(def: AgentDefinition, state: State<'_, RuntimeState>) -> Result<(), String> {
    state.save_agent_definition(&def)
}

#[tauri::command]
pub fn list_archived_agents() -> Result<Vec<AgentDefinition>, String> {
    agents::list_archived_agents()
}

/// 列出 Agent 目录内可编辑的 Markdown（AGENTS.md / memory/*.md）。
#[tauri::command]
pub fn list_agent_files(agent_name: String) -> Result<Vec<String>, String> {
    agents::list_agent_md_files(&agent_name)
}

/// 读取 Agent 目录内的可编辑 Markdown（AGENTS.md / memory/*.md）。
#[tauri::command]
pub fn read_agent_file(agent_name: String, rel_path: String) -> Result<String, String> {
    agents::read_agent_file(&agent_name, &rel_path)
}

/// 写入 Agent 目录内的可编辑 Markdown（AGENTS.md / memory/*.md）。
#[tauri::command]
pub fn write_agent_file(
    agent_name: String,
    rel_path: String,
    content: String,
    state: State<'_, RuntimeState>,
) -> Result<(), String> {
    state.write_agent_file(&agent_name, &rel_path, &content)
}

#[tauri::command]
pub fn get_settings() -> Settings {
    settings::load_settings()
}

#[tauri::command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    settings::save_settings(&settings)
}

// ============ 技能（av store 与 Agent 契约）============

/// 全局技能 store（`~/.av/skills`）一览：已安装版本、来源与内容存在性。
#[tauri::command]
pub fn skill_library() -> Result<SkillLibrary, String> {
    skill_center::store_skills()
}

/// 某个 Agent 的技能启用视图（`[resources.skills].use` + 解析状态）。
#[tauri::command]
pub fn agent_skills(agent_name: String) -> Result<AgentSkills, String> {
    skill_center::agent_skills(&agent_name)
}

/// 启用/禁用一个 store 技能（写契约 `use` 与同层 `agent.lock`）。
#[tauri::command]
pub fn set_skill_enabled(
    agent_name: String,
    name: String,
    enabled: bool,
) -> Result<AgentSkills, String> {
    skill_center::set_skill_enabled(&agent_name, &name, enabled)
}

/// 重算 store 内容哈希。要读全部技能内容，放到阻塞线程池执行。
#[tauri::command]
pub async fn verify_store_skills(names: Option<Vec<String>>) -> Result<Vec<SkillVerify>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        skill_center::verify_store_skills(names.as_deref())
    })
    .await
    .map_err(|error| format!("技能校验任务失败：{error}"))?
}

/// 安装技能到全局 store（只装不启用）。git 源要联网，放到阻塞线程池执行。
#[tauri::command]
pub async fn install_skill(
    source: String,
    skills: Option<Vec<String>>,
    all: bool,
    dry_run: bool,
) -> Result<SkillInstallReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        skill_center::install_skill(&source, skills.as_deref(), all, dry_run)
    })
    .await
    .map_err(|error| format!("技能安装任务失败：{error}"))?
}

/// 从 store 移除一个技能的全部版本（仍被 Agent 声明时拒绝）。
#[tauri::command]
pub fn remove_store_skill(name: String) -> Result<usize, String> {
    skill_center::remove_store_skill(&name)
}

/// 同步/修复声明技能（可能联网取 git）。放到阻塞线程池执行。
#[tauri::command]
pub async fn sync_agent_skills(
    agent_name: String,
    repair: bool,
) -> Result<SkillSyncReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        skill_center::sync_agent_skills(&agent_name, repair)
    })
    .await
    .map_err(|error| format!("技能同步任务失败：{error}"))?
}

/// 一个技能在各个 Agent 上的启用情况（技能库详情用）。
#[tauri::command]
pub fn skill_usage(name: String) -> Result<Vec<SkillUsage>, String> {
    skill_center::skill_usage(&name)
}
