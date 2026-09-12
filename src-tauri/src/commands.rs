//! Tauri 命令层 —— 只是把 pipi-core 的能力暴露给前端，不含业务逻辑。

use pipi_core::agents::{self, AgentDefinition, PermissionsConfig};
use pipi_core::catalog::ModelCatalog;
use pipi_core::settings::{self, Settings};
use pipi_core::types::Model;

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
pub fn save_agent(def: AgentDefinition) -> Result<(), String> {
    agents::save_agent(&def)
}

#[tauri::command]
pub fn get_settings() -> Settings {
    settings::load_settings()
}

#[tauri::command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    settings::save_settings(&settings)
}
