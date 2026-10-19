//! Tauri 命令层（Agent / 设置 / 技能）—— 只做 IPC 转发，
//! 逻辑与参数结构体在 `pipi_core::commands`（宿主命令的单一事实来源）。

use pipi_core::commands::{
    AgentSkillsArgs, CreateAgentArgs, ForumPostArgs, ForumReadArgs, ForumReplyArgs,
    ForumSearchArgs, ForumThread, ForumThreadsArgs, InstallSkillArgs, ListAgentFilesArgs,
    ListAgentsArgs, ModelCatalogArgs, ReadAgentFileArgs, RemoveStoreSkillArgs, SaveAgentArgs,
    SaveSettingsArgs, SetSkillEnabledArgs, SkillUsageArgs, SyncAgentSkillsArgs,
    VerifyStoreSkillsArgs, WriteAgentFileArgs,
};
use pipi_core::runtime::RuntimeState;
use pipi_core::settings::Settings;
use pipi_protocol::forum::{Post, SearchHit, Thread, ThreadSummary};
use pipi_provider::catalog::ModelCatalog;
use pipi_venv::agents::AgentDefinition;
use pipi_venv::skills::{
    AgentSkills, SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify,
};
use tauri::State;

use crate::invoke_args::InvokeArgs;

#[tauri::command]
pub fn list_agents(args: InvokeArgs<ListAgentsArgs>) -> Result<Vec<AgentDefinition>, String> {
    pipi_core::commands::list_agents(args.0)
}

/// 模型目录（models.dev + 本地缓存）。必须 async：它要联网，Tauri 会把它放到
/// 自己的异步运行时上执行，从而拿到 IO 驱动（同步命令跑在 GTK 主线程，那里没有）。
#[tauri::command]
pub async fn model_catalog(args: InvokeArgs<ModelCatalogArgs>) -> Result<ModelCatalog, String> {
    pipi_core::commands::model_catalog(args.0).await
}

#[tauri::command]
pub fn create_agent(args: InvokeArgs<CreateAgentArgs>) -> Result<AgentDefinition, String> {
    pipi_core::commands::create_agent(args.0)
}

#[tauri::command]
pub fn save_agent(
    state: State<'_, RuntimeState>,
    args: InvokeArgs<SaveAgentArgs>,
) -> Result<(), String> {
    pipi_core::commands::save_agent(&state, args.0)
}

/// 列出 Agent 目录内可编辑的 Markdown（契约声明的指令文件 + 记忆文件）。
#[tauri::command]
pub fn list_agent_files(
    args: InvokeArgs<ListAgentFilesArgs>,
) -> Result<pipi_venv::agents::AgentEditableFiles, String> {
    pipi_core::commands::list_agent_files(args.0)
}

/// 读取 Agent 目录内的可编辑 Markdown（harness/AGENTS.md / harness/memory/*.md）。
#[tauri::command]
pub fn read_agent_file(args: InvokeArgs<ReadAgentFileArgs>) -> Result<String, String> {
    pipi_core::commands::read_agent_file(args.0)
}

/// 写入 Agent 目录内的可编辑 Markdown（harness/AGENTS.md / harness/memory/*.md）。
#[tauri::command]
pub fn write_agent_file(
    state: State<'_, RuntimeState>,
    args: InvokeArgs<WriteAgentFileArgs>,
) -> Result<(), String> {
    pipi_core::commands::write_agent_file(&state, args.0)
}

#[tauri::command]
pub fn get_settings() -> Settings {
    pipi_core::commands::get_settings()
}

#[tauri::command]
pub fn save_settings(args: InvokeArgs<SaveSettingsArgs>) -> Result<(), String> {
    pipi_core::commands::save_settings(args.0)
}

// ============ 技能（av store 与 Agent 契约）============

/// 全局技能 store（`~/.av/skills`）一览：已安装版本、来源与内容存在性。
#[tauri::command]
pub fn skill_library() -> Result<SkillLibrary, String> {
    pipi_core::commands::skill_library()
}

/// 某个 Agent 的技能启用视图（`[resources.skills].use` + 解析状态）。
#[tauri::command]
pub fn agent_skills(args: InvokeArgs<AgentSkillsArgs>) -> Result<AgentSkills, String> {
    pipi_core::commands::agent_skills(args.0)
}

/// 启用/禁用一个 store 技能（写契约 `use` 与同层 `agent.lock`）。
#[tauri::command]
pub fn set_skill_enabled(args: InvokeArgs<SetSkillEnabledArgs>) -> Result<AgentSkills, String> {
    pipi_core::commands::set_skill_enabled(args.0)
}

/// 重算 store 内容哈希。要读全部技能内容，放到阻塞线程池执行。
#[tauri::command]
pub async fn verify_store_skills(
    args: InvokeArgs<VerifyStoreSkillsArgs>,
) -> Result<Vec<SkillVerify>, String> {
    pipi_core::commands::verify_store_skills(args.0).await
}

/// 安装技能到全局 store（只装不启用）。git 源要联网，放到阻塞线程池执行。
#[tauri::command]
pub async fn install_skill(
    args: InvokeArgs<InstallSkillArgs>,
) -> Result<SkillInstallReport, String> {
    pipi_core::commands::install_skill(args.0).await
}

/// 从 store 移除一个技能的全部版本（仍被 Agent 声明时拒绝）。
#[tauri::command]
pub fn remove_store_skill(args: InvokeArgs<RemoveStoreSkillArgs>) -> Result<usize, String> {
    pipi_core::commands::remove_store_skill(args.0)
}

/// 同步/修复声明技能（可能联网取 git）。放到阻塞线程池执行。
#[tauri::command]
pub async fn sync_agent_skills(
    args: InvokeArgs<SyncAgentSkillsArgs>,
) -> Result<SkillSyncReport, String> {
    pipi_core::commands::sync_agent_skills(args.0).await
}

/// 一个技能在各个 Agent 上的启用情况（技能库详情用）。
#[tauri::command]
pub fn skill_usage(args: InvokeArgs<SkillUsageArgs>) -> Result<Vec<SkillUsage>, String> {
    pipi_core::commands::skill_usage(args.0)
}

#[tauri::command]
pub fn forum_threads(args: InvokeArgs<ForumThreadsArgs>) -> Result<Vec<ThreadSummary>, String> {
    pipi_core::commands::forum_threads(args.0)
}

#[tauri::command]
pub fn forum_read(args: InvokeArgs<ForumReadArgs>) -> Result<ForumThread, String> {
    pipi_core::commands::forum_read(args.0)
}

#[tauri::command]
pub fn forum_search(args: InvokeArgs<ForumSearchArgs>) -> Result<Vec<SearchHit>, String> {
    pipi_core::commands::forum_search(args.0)
}

#[tauri::command]
pub fn forum_post(args: InvokeArgs<ForumPostArgs>) -> Result<Thread, String> {
    pipi_core::commands::forum_post(args.0)
}

#[tauri::command]
pub fn forum_reply(args: InvokeArgs<ForumReplyArgs>) -> Result<Post, String> {
    pipi_core::commands::forum_reply(args.0)
}
