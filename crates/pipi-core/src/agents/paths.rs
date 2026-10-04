use std::fs;
use std::path::PathBuf;

use pipi_agent::session::ARCHIVE_DIR;
use pipi_tools::fs::ensure_real_directory;

/// 所有 Agent 数据的根：`~/.pipi/agents`（路径布局由 av 统一定义）。
pub fn agents_dir() -> Option<PathBuf> {
    av::paths::agents_root()
}

/// 校验 Agent 名称必须是单一、稳定的目录名，禁止路径分隔符与 `..` 穿越。
pub fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Agent 名称不能为空".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("名称只能包含字母、数字、- 和 _".into());
    }
    Ok(())
}

pub(super) fn manifest_name_matches_directory(directory_name: &str, manifest_name: &str) -> bool {
    directory_name == manifest_name && validate_agent_name(manifest_name).is_ok()
}

pub(super) fn checked_agents_root() -> Result<PathBuf, String> {
    let agents = agents_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    let pipi = agents
        .parent()
        .ok_or_else(|| "无法定位 .pipi 目录".to_string())?;
    ensure_real_directory(pipi, ".pipi")?;
    ensure_real_directory(&agents, "agents")?;
    Ok(agents)
}

pub(super) fn checked_agent_dir(name: &str) -> Result<PathBuf, String> {
    validate_agent_name(name)?;
    let dir = checked_agents_root()?.join(name);
    ensure_real_directory(&dir, "Agent 目录")?;
    Ok(dir)
}

pub fn agent_dir(name: &str) -> Option<PathBuf> {
    checked_agent_dir(name).ok()
}

/// 展开 `~` 前缀（仅支持 `~` 与 `~/...`；规则复用 av 的同一份实现）。
pub fn expand_tilde(path: &str) -> String {
    av::discovery::expand_tilde(path)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn checked_archive_dir() -> Result<PathBuf, String> {
    Ok(checked_agents_root()?.join(ARCHIVE_DIR))
}

pub(super) fn checked_archived_agent_dir(name: &str) -> Result<PathBuf, String> {
    validate_agent_name(name)?;
    let dir = checked_archive_dir()?.join(name);
    ensure_real_directory(&dir, "归档 Agent 目录")?;
    Ok(dir)
}

/// 确保路径里的 agent 目录存在（从文件系统恢复定义时用）。
pub fn ensure_agent_dir(name: &str) -> Result<PathBuf, String> {
    let dir = checked_agent_dir(name)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}
