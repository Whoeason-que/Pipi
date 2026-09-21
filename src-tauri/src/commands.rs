//! Tauri 命令层 —— 只是把 pipi-core 的能力暴露给前端，不含业务逻辑。

use pipi_core::agents::{self, AgentDefinition, PermissionsConfig};
use pipi_core::catalog::ModelCatalog;
use pipi_core::runtime::RuntimeState;
use pipi_core::settings::{self, Settings};
use pipi_core::types::Model;
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
