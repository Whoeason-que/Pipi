//! Tauri 命令层 —— 只是把 pipi-core 的能力暴露给前端，不含业务逻辑。

use pipi_core::agents::{self, AgentDefinition, PermissionsConfig};
use pipi_core::settings::{self, Settings};

#[tauri::command]
pub fn list_agents() -> Result<Vec<AgentDefinition>, String> {
    agents::list_agents()
}

#[tauri::command]
pub fn create_agent(
    name: String,
    description: String,
    workspace: Option<String>,
    permissions: Option<PermissionsConfig>,
) -> Result<AgentDefinition, String> {
    agents::create_agent(&name, &description, workspace.as_deref(), permissions)
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
