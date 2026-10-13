//! 宿主命令的单一事实来源。
//!
//! 47 个命令的名称（[`COMMANDS`]）、参数结构体、逻辑与 JSON 分发表都在这里；
//! `crates/pipi`（桌面壳）与 `server` 特性里的 Web 宿主只做 transport：前者把
//! 每个命令转发成 `#[tauri::command]` 薄壳，后者调用 [`dispatch`]。两边不再各自
//! 复制命令表，也不再手写 JSON 参数解析。
//!
//! 参数结构体统一 `#[serde(rename_all = "camelCase")]`，字段名、类型与可选性
//! 就是前端 `src/ipc.ts` 的宿主命令契约（由 `tests/ipc-contract.test.ts` 核对）。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::{self, Settings};
use pipi_agent::stats::SessionStats;
use pipi_protocol::permissions::PermissionsConfig;
use pipi_protocol::session::{SessionImportReport, SessionSummary};
use pipi_protocol::{BackgroundTaskSnapshot, Message, Model};
use pipi_provider::catalog::ModelCatalog;
use pipi_venv::agents::{self, AgentDefinition, AgentEditableFiles};
use pipi_venv::skills::{
    AgentSkills, SkillInstallReport, SkillLibrary, SkillSyncReport, SkillUsage, SkillVerify,
};

use crate::approval::ApprovalDecision;
use crate::events::EventEmitter;
use crate::runtime::{self, RuntimeState, SessionInfo, SessionSnapshot, agent_repo};

/// 全部宿主命令名，顺序与前端 `src/ipc.ts` 的 `CommandMap` 一致。
pub const COMMANDS: &[&str] = &[
    "list_agents",
    "model_catalog",
    "create_agent",
    "save_agent",
    "list_agent_files",
    "read_agent_file",
    "write_agent_file",
    "get_settings",
    "save_settings",
    "list_sessions",
    "open_session",
    "session_info",
    "session_infos",
    "open_workbench_session",
    "stop_run",
    "query_background_tasks",
    "manage_background_task",
    "new_session",
    "send_prompt",
    "compact_now",
    "steer",
    "resolve_approval",
    "set_session_model",
    "session_messages",
    "session_stats",
    "fork_session",
    "archive_agent",
    "restore_agent",
    "delete_agent",
    "import_sessions",
    "archive_session",
    "restore_session",
    "delete_session",
    "skill_library",
    "agent_skills",
    "set_skill_enabled",
    "verify_store_skills",
    "install_skill",
    "remove_store_skill",
    "sync_agent_skills",
    "skill_usage",
];

// ============ Agent ============

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsArgs {
    pub archived: Option<bool>,
}

/// 列出活跃或归档的 Agent。活跃 Agent 在首次启动时自动播种默认 Agent（Pipi）。
pub fn list_agents(args: ListAgentsArgs) -> Result<Vec<AgentDefinition>, String> {
    if args.archived.unwrap_or(false) {
        agents::list_archived_agents()
    } else {
        agents::list_agents_bootstrapped()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogArgs {
    pub refresh: Option<bool>,
}

/// 模型目录（models.dev + 本地缓存）。
pub async fn model_catalog(args: ModelCatalogArgs) -> Result<ModelCatalog, String> {
    pipi_provider::catalog::load_catalog(
        args.refresh.unwrap_or(false),
        pipi_venv::av::paths::catalog_cache_path(),
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentArgs {
    pub name: String,
    pub description: String,
    pub workspace: Option<String>,
    pub permissions: Option<PermissionsConfig>,
    pub model: Option<String>,
    pub provider: Option<Model>,
}

pub fn create_agent(args: CreateAgentArgs) -> Result<AgentDefinition, String> {
    let CreateAgentArgs {
        name,
        description,
        workspace,
        permissions,
        model,
        provider,
    } = args;
    agents::create_agent(
        &name,
        &description,
        workspace.as_deref(),
        permissions,
        model.as_deref(),
        provider,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentArgs {
    pub def: AgentDefinition,
}

pub fn save_agent(runtime: &RuntimeState, args: SaveAgentArgs) -> Result<(), String> {
    runtime.save_agent_definition(&args.def)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentFilesArgs {
    pub agent_name: String,
}

/// 列出 Agent 目录内可编辑的 Markdown：契约声明的指令文件 + 记忆目录下的 `.md`
/// （位置来自 agent.toml 的 `[resources]`，不是硬编码目录）。
pub fn list_agent_files(args: ListAgentFilesArgs) -> Result<AgentEditableFiles, String> {
    agents::list_agent_editable_files(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadAgentFileArgs {
    pub agent_name: String,
    pub rel_path: String,
}

/// 读取 Agent 目录内的可编辑 Markdown（harness/AGENTS.md / harness/memory/*.md）。
pub fn read_agent_file(args: ReadAgentFileArgs) -> Result<String, String> {
    agents::read_agent_file(&args.agent_name, &args.rel_path)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteAgentFileArgs {
    pub agent_name: String,
    pub rel_path: String,
    pub content: String,
}

/// 写入 Agent 目录内的可编辑 Markdown（harness/AGENTS.md / harness/memory/*.md）。
pub fn write_agent_file(runtime: &RuntimeState, args: WriteAgentFileArgs) -> Result<(), String> {
    runtime.write_agent_file(&args.agent_name, &args.rel_path, &args.content)
}

pub fn get_settings() -> Settings {
    settings::load_settings()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsArgs {
    pub settings: Settings,
}

pub fn save_settings(args: SaveSettingsArgs) -> Result<(), String> {
    settings::save_settings(&args.settings)
}

// ============ 会话 ============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsArgs {
    pub agent_name: String,
    pub archived: Option<bool>,
}

pub fn list_sessions(
    runtime: &RuntimeState,
    args: ListSessionsArgs,
) -> Result<Vec<SessionSummary>, String> {
    if args.archived.unwrap_or(false) {
        runtime.list_archived_sessions(&args.agent_name)
    } else {
        runtime::list_sessions(&args.agent_name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn open_session(
    runtime: &RuntimeState,
    args: OpenSessionArgs,
) -> Result<SessionSnapshot, String> {
    runtime.open_session(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn session_info(
    runtime: &RuntimeState,
    args: SessionInfoArgs,
) -> Result<Option<SessionInfo>, String> {
    runtime.session_info(&args.agent_name, &args.session_id)
}

/// 所有打开中的会话：前端据此知道「哪些 Agent 正在跑」（多 Agent 并发下运行态是集合）。
pub fn session_infos(runtime: &RuntimeState) -> Result<Vec<SessionInfo>, String> {
    runtime.session_infos()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWorkbenchSessionArgs {
    pub agent_name: String,
    pub reset: Option<bool>,
}

/// 打开或重置工作台测试会话。
pub fn open_workbench_session(
    runtime: &RuntimeState,
    args: OpenWorkbenchSessionArgs,
) -> Result<SessionInfo, String> {
    if args.reset.unwrap_or(false) {
        runtime.reset_test_session(&args.agent_name)
    } else {
        runtime.ensure_test_session(&args.agent_name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopRunArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn stop_run(runtime: &RuntimeState, args: StopRunArgs) -> Result<(), String> {
    runtime.stop_run(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryBackgroundTasksArgs {
    pub agent_name: String,
    pub session_id: String,
    pub job_id: Option<String>,
    pub after_seq: Option<u64>,
    pub wait_ms: Option<u64>,
    pub include_completed: Option<bool>,
    pub limit: Option<usize>,
}

pub async fn query_background_tasks(
    runtime: &RuntimeState,
    args: QueryBackgroundTasksArgs,
) -> Result<Vec<BackgroundTaskSnapshot>, String> {
    let QueryBackgroundTasksArgs {
        agent_name,
        session_id,
        job_id,
        after_seq,
        wait_ms,
        include_completed,
        limit,
    } = args;
    runtime
        .query_background_tasks(
            &agent_name,
            &session_id,
            job_id,
            after_seq.unwrap_or(0),
            wait_ms.unwrap_or(0),
            include_completed.unwrap_or(true),
            limit.unwrap_or(20),
        )
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManageBackgroundTaskArgs {
    pub agent_name: String,
    pub session_id: String,
    pub job_id: String,
    pub action: String,
    pub data: Option<String>,
}

pub async fn manage_background_task(
    runtime: &RuntimeState,
    args: ManageBackgroundTaskArgs,
) -> Result<BackgroundTaskSnapshot, String> {
    runtime
        .manage_background_task(
            &args.agent_name,
            &args.session_id,
            &args.job_id,
            &args.action,
            args.data,
        )
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionArgs {
    pub agent_name: String,
}

pub fn new_session(runtime: &RuntimeState, args: NewSessionArgs) -> Result<(), String> {
    runtime.new_session(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPromptArgs {
    pub agent_name: String,
    pub session_id: Option<String>,
    pub prompt: String,
    pub model: Option<Model>,
}

pub fn send_prompt(
    runtime: &RuntimeState,
    emitter: EventEmitter,
    args: SendPromptArgs,
) -> Result<SessionInfo, String> {
    runtime.send_prompt(
        &args.agent_name,
        args.session_id.as_deref(),
        &args.prompt,
        args.model,
        emitter,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactNowArgs {
    pub agent_name: String,
    pub session_id: String,
}

/// 手动压缩当前会话（跳过阈值预检，走与自动压缩相同的分叉/归档路径）。
pub fn compact_now(
    runtime: &RuntimeState,
    emitter: EventEmitter,
    args: CompactNowArgs,
) -> Result<(), String> {
    runtime.compact_now(&args.agent_name, &args.session_id, emitter)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerArgs {
    pub agent_name: String,
    pub session_id: String,
    pub message: String,
}

/// 运行中插话（steering）：注入当前运行的下一轮上下文。
pub fn steer(runtime: &RuntimeState, args: SteerArgs) -> Result<(), String> {
    runtime.steer(&args.agent_name, &args.session_id, &args.message)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveApprovalArgs {
    pub request_id: String,
    pub decision: String,
}

/// 回传 bash 命令审批请求的用户决定。
pub fn resolve_approval(runtime: &RuntimeState, args: ResolveApprovalArgs) -> Result<(), String> {
    runtime.resolve_approval(&args.request_id, args.decision.parse::<ApprovalDecision>()?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionModelArgs {
    pub agent_name: String,
    pub session_id: String,
    pub model: Option<Model>,
}

pub fn set_session_model(runtime: &RuntimeState, args: SetSessionModelArgs) -> Result<(), String> {
    runtime.set_session_model(&args.agent_name, &args.session_id, args.model)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessagesArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub async fn session_messages(
    runtime: &RuntimeState,
    args: SessionMessagesArgs,
) -> Result<Vec<Message>, String> {
    runtime
        .session_messages(&args.agent_name, &args.session_id)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatsArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn session_stats(
    runtime: &RuntimeState,
    args: SessionStatsArgs,
) -> Result<SessionStats, String> {
    runtime.session_stats(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionArgs {
    pub agent_name: String,
    pub session_id: String,
    pub up_to_entry_id: Option<String>,
}

pub fn fork_session(runtime: &RuntimeState, args: ForkSessionArgs) -> Result<SessionInfo, String> {
    runtime.fork_session(
        &args.agent_name,
        &args.session_id,
        args.up_to_entry_id.as_deref(),
    )
}

// ============ 归档 / 恢复 / 删除（Agent 与会话）============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveAgentArgs {
    pub name: String,
}

pub fn archive_agent(runtime: &RuntimeState, args: ArchiveAgentArgs) -> Result<(), String> {
    runtime.archive_agent(&args.name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreAgentArgs {
    pub name: String,
}

pub fn restore_agent(runtime: &RuntimeState, args: RestoreAgentArgs) -> Result<(), String> {
    let _ = runtime;
    agents::restore_agent(&args.name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAgentArgs {
    pub name: String,
    pub archived: Option<bool>,
}

pub fn delete_agent(runtime: &RuntimeState, args: DeleteAgentArgs) -> Result<(), String> {
    if args.archived.unwrap_or(false) {
        agents::delete_archived_agent(&args.name)
    } else {
        runtime.delete_agent(&args.name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionsArgs {
    pub agent_name: String,
}

/// 把旧 JSONL 会话导入 git（逐条校验、幂等、原文件改名保留）。
///
/// 占用检查与脱离槽位在锁内同步做（快），读全部旧会话并 spawn git 的部分放到
/// 阻塞线程池执行（与技能安装同一条约定）。
pub async fn import_sessions(
    runtime: &RuntimeState,
    args: ImportSessionsArgs,
) -> Result<SessionImportReport, String> {
    runtime.prepare_session_import(&args.agent_name)?;
    let agent_name = args.agent_name.clone();
    tokio::task::spawn_blocking(move || {
        let def = agents::load_agent(&agent_name)?;
        let legacy_dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        Ok(pipi_session::import_legacy_sessions(
            &agent_repo(&def)?,
            &legacy_dir,
        ))
    })
    .await
    .map_err(|error| format!("会话导入任务失败：{error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSessionArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn archive_session(runtime: &RuntimeState, args: ArchiveSessionArgs) -> Result<(), String> {
    runtime.archive_session(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSessionArgs {
    pub agent_name: String,
    pub session_id: String,
}

pub fn restore_session(runtime: &RuntimeState, args: RestoreSessionArgs) -> Result<(), String> {
    runtime.restore_session(&args.agent_name, &args.session_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub agent_name: String,
    pub session_id: String,
    pub archived: Option<bool>,
}

pub fn delete_session(runtime: &RuntimeState, args: DeleteSessionArgs) -> Result<(), String> {
    if args.archived.unwrap_or(false) {
        runtime.delete_archived_session(&args.agent_name, &args.session_id)
    } else {
        runtime.delete_session(&args.agent_name, &args.session_id)
    }
}

// ============ 技能（av store 与 Agent 契约）============

/// 全局技能 store（`~/.av/skills`）一览：已安装版本、来源与内容存在性。
pub fn skill_library() -> Result<SkillLibrary, String> {
    pipi_venv::skills::store_skills()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillsArgs {
    pub agent_name: String,
}

/// 某个 Agent 的技能启用视图（`[resources.skills].use` + 解析状态）。
pub fn agent_skills(args: AgentSkillsArgs) -> Result<AgentSkills, String> {
    pipi_venv::skills::agent_skills(&args.agent_name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSkillEnabledArgs {
    pub agent_name: String,
    pub name: String,
    pub enabled: bool,
}

/// 启用/禁用一个 store 技能（写契约 `use` 与同层 `agent.lock`）。
pub fn set_skill_enabled(args: SetSkillEnabledArgs) -> Result<AgentSkills, String> {
    pipi_venv::skills::set_skill_enabled(&args.agent_name, &args.name, args.enabled)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyStoreSkillsArgs {
    pub names: Option<Vec<String>>,
}

/// 重算 store 内容哈希。要读全部技能内容，放到阻塞线程池执行。
pub async fn verify_store_skills(args: VerifyStoreSkillsArgs) -> Result<Vec<SkillVerify>, String> {
    tokio::task::spawn_blocking(move || {
        pipi_venv::skills::verify_store_skills(args.names.as_deref())
    })
    .await
    .map_err(|error| format!("技能校验任务失败：{error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSkillArgs {
    pub source: String,
    pub skills: Option<Vec<String>>,
    pub all: bool,
    pub dry_run: bool,
}

/// 安装技能到全局 store（只装不启用）。git 源要联网，放到阻塞线程池执行。
pub async fn install_skill(args: InstallSkillArgs) -> Result<SkillInstallReport, String> {
    tokio::task::spawn_blocking(move || {
        pipi_venv::skills::install_skill(
            &args.source,
            args.skills.as_deref(),
            args.all,
            args.dry_run,
        )
    })
    .await
    .map_err(|error| format!("技能安装任务失败：{error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveStoreSkillArgs {
    pub name: String,
}

/// 从 store 移除一个技能的全部版本（仍被 Agent 声明时拒绝）。
pub fn remove_store_skill(args: RemoveStoreSkillArgs) -> Result<usize, String> {
    pipi_venv::skills::remove_store_skill(&args.name)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAgentSkillsArgs {
    pub agent_name: String,
    pub repair: bool,
}

/// 同步/修复声明技能（可能联网取 git）。放到阻塞线程池执行。
pub async fn sync_agent_skills(args: SyncAgentSkillsArgs) -> Result<SkillSyncReport, String> {
    tokio::task::spawn_blocking(move || {
        pipi_venv::skills::sync_agent_skills(&args.agent_name, args.repair)
    })
    .await
    .map_err(|error| format!("技能同步任务失败：{error}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUsageArgs {
    pub name: String,
}

/// 一个技能在各个 Agent 上的启用情况（技能库详情用）。
pub fn skill_usage(args: SkillUsageArgs) -> Result<Vec<SkillUsage>, String> {
    pipi_venv::skills::skill_usage(&args.name)
}

// ============ Web / JSON transport 入口 ============

/// Web 宿主（以及任何 JSON transport）的唯一入口：按命令名解析参数并调用对应函数。
pub async fn dispatch(
    runtime: &RuntimeState,
    emitter: EventEmitter,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    match name {
        "list_agents" => to_value(list_agents(parse(args)?)?),
        "model_catalog" => to_value(model_catalog(parse(args)?).await?),
        "create_agent" => to_value(create_agent(parse(args)?)?),
        "save_agent" => to_value(save_agent(runtime, parse(args)?)?),
        "list_agent_files" => to_value(list_agent_files(parse(args)?)?),
        "read_agent_file" => to_value(read_agent_file(parse(args)?)?),
        "write_agent_file" => to_value(write_agent_file(runtime, parse(args)?)?),
        "get_settings" => to_value(get_settings()),
        "save_settings" => to_value(save_settings(parse(args)?)?),
        "list_sessions" => to_value(list_sessions(runtime, parse(args)?)?),
        "open_session" => to_value(open_session(runtime, parse(args)?)?),
        "session_info" => to_value(session_info(runtime, parse(args)?)?),
        "session_infos" => to_value(session_infos(runtime)?),
        "open_workbench_session" => to_value(open_workbench_session(runtime, parse(args)?)?),
        "stop_run" => to_value(stop_run(runtime, parse(args)?)?),
        "query_background_tasks" => to_value(query_background_tasks(runtime, parse(args)?).await?),
        "manage_background_task" => to_value(manage_background_task(runtime, parse(args)?).await?),
        "new_session" => to_value(new_session(runtime, parse(args)?)?),
        "send_prompt" => to_value(send_prompt(runtime, emitter, parse(args)?)?),
        "compact_now" => to_value(compact_now(runtime, emitter, parse(args)?)?),
        "steer" => to_value(steer(runtime, parse(args)?)?),
        "resolve_approval" => to_value(resolve_approval(runtime, parse(args)?)?),
        "set_session_model" => to_value(set_session_model(runtime, parse(args)?)?),
        "session_messages" => to_value(session_messages(runtime, parse(args)?).await?),
        "session_stats" => to_value(session_stats(runtime, parse(args)?)?),
        "fork_session" => to_value(fork_session(runtime, parse(args)?)?),
        "archive_agent" => to_value(archive_agent(runtime, parse(args)?)?),
        "restore_agent" => to_value(restore_agent(runtime, parse(args)?)?),
        "delete_agent" => to_value(delete_agent(runtime, parse(args)?)?),
        "import_sessions" => to_value(import_sessions(runtime, parse(args)?).await?),
        "archive_session" => to_value(archive_session(runtime, parse(args)?)?),
        "restore_session" => to_value(restore_session(runtime, parse(args)?)?),
        "delete_session" => to_value(delete_session(runtime, parse(args)?)?),
        "skill_library" => to_value(skill_library()?),
        "agent_skills" => to_value(agent_skills(parse(args)?)?),
        "set_skill_enabled" => to_value(set_skill_enabled(parse(args)?)?),
        "verify_store_skills" => to_value(verify_store_skills(parse(args)?).await?),
        "install_skill" => to_value(install_skill(parse(args)?).await?),
        "remove_store_skill" => to_value(remove_store_skill(parse(args)?)?),
        "sync_agent_skills" => to_value(sync_agent_skills(parse(args)?).await?),
        "skill_usage" => to_value(skill_usage(parse(args)?)?),
        other => Err(format!("未知命令：{other}")),
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() {
        serde_json::json!({})
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| format!("命令参数无效：{error}"))
}

fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| format!("命令结果序列化失败：{error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn commands_are_registered_once() {
        let unique: HashSet<&str> = COMMANDS.iter().copied().collect();
        assert_eq!(unique.len(), COMMANDS.len(), "COMMANDS 存在重复命令名");
        assert_eq!(
            COMMANDS.len(),
            41,
            "宿主命令数应为 41（前端 CommandMap 同步）"
        );
    }

    #[tokio::test]
    async fn unknown_command_is_rejected() {
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let emitter: EventEmitter = Arc::new(|_| {});
        let error = dispatch(&runtime, emitter, "no_such_command", Value::Null)
            .await
            .expect_err("未知命令必须报错");
        assert!(
            error.contains("no_such_command"),
            "错误信息应包含命令名：{error}"
        );
    }

    #[tokio::test]
    async fn dispatch_parses_flat_camel_case_args() {
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let emitter: EventEmitter = Arc::new(|_| {});
        let value = dispatch(
            &runtime,
            emitter,
            "list_agents",
            serde_json::json!({ "archived": false }),
        )
        .await
        .expect("list_agents 应当成功");
        assert!(value.is_array());
    }
}
