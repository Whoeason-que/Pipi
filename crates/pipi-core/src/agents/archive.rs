use std::fs;

use super::paths::{
    checked_agent_dir, checked_agents_root, checked_archive_dir, checked_archived_agent_dir,
};
use super::registry::scan_agents;
use super::types::AgentDefinition;

// ============ 归档 / 恢复 / 删除 ============
// 归档 = 移动到 `agents/.archive/<name>/`；删除 = 彻底删目录。
// 都是纯文件操作：移动失败、目录不存在都直接报中文错误。
// 调用方（应用层）负责先做「会话占用」检查，并在无占用时释放空闲会话槽，
// 再移动 Agent 目录。

/// 列出已归档 Agent（`~/.pipi/agents/.archive/` 下）。
pub fn list_archived_agents() -> Result<Vec<AgentDefinition>, String> {
    let dir = checked_archive_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(scan_agents(&dir))
}

/// 归档 Agent：把 `agents/<name>/` 整体移入 `agents/.archive/<name>/`。
pub fn archive_agent(name: &str) -> Result<(), String> {
    let src = checked_agent_dir(name)?;
    let archive_dir = checked_archive_dir()?;
    fs::create_dir_all(&archive_dir).map_err(|e| format!("无法创建归档目录: {e}"))?;
    let dst = archive_dir.join(name);
    if dst.exists() {
        return Err(format!("归档区已存在同名 Agent「{name}」"));
    }
    fs::rename(&src, &dst).map_err(|e| format!("归档 Agent「{name}」失败: {e}"))
}

/// 恢复归档 Agent：移回 `agents/<name>/`。
pub fn restore_agent(name: &str) -> Result<(), String> {
    let src = checked_archived_agent_dir(name)?;
    let root = checked_agents_root()?;
    let dst = root.join(name);
    if dst.exists() {
        return Err(format!("活跃区已存在同名 Agent「{name}」"));
    }
    fs::rename(&src, &dst).map_err(|e| format!("恢复 Agent「{name}」失败: {e}"))
}

/// 彻底删除 Agent（含其目录下的 session、workspace、skills、memory）。
/// 不可恢复；调用方应先在 UI 层做确认。
pub fn delete_agent(name: &str) -> Result<(), String> {
    let dir = checked_agent_dir(name)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("删除 Agent「{name}」失败: {e}"))
}

/// 彻底删除已归档 Agent（`agents/.archive/<name>/`）。不可恢复。
pub fn delete_archived_agent(name: &str) -> Result<(), String> {
    let dir = checked_archived_agent_dir(name)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("删除归档 Agent「{name}」失败: {e}"))
}
