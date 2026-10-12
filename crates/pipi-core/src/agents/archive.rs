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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{agents_dir, create_agent, list_agents};
    use pipi_protocol::session::ARCHIVE_DIR;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn agent_archive_restore_delete_lifecycle() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-home-arch-{}", pipi_protocol::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let name = "arch-lifecycle";
        create_agent(name, "归档生命周期", None, None, None, None).unwrap();
        assert!(list_agents().unwrap().iter().any(|a| a.name == name));
        assert!(list_archived_agents().unwrap().is_empty());

        // 归档：活跃列表消失、归档列表出现、目录真的移动了
        archive_agent(name).unwrap();
        assert!(!list_agents().unwrap().iter().any(|a| a.name == name));
        assert!(
            list_archived_agents()
                .unwrap()
                .iter()
                .any(|a| a.name == name)
        );
        let dir = agents_dir().unwrap();
        assert!(dir.join(ARCHIVE_DIR).join(name).is_dir());
        assert!(!dir.join(name).exists());

        // 重复归档同一名字的 Agent → 该名字现在已不在活跃区 → 报错
        assert!(archive_agent(name).is_err());

        // 恢复：回到活跃区
        restore_agent(name).unwrap();
        assert!(list_agents().unwrap().iter().any(|a| a.name == name));
        assert!(
            !list_archived_agents()
                .unwrap()
                .iter()
                .any(|a| a.name == name)
        );

        // 恢复一个不存在的归档 Agent → 报错
        assert!(restore_agent("no-such-archived-agent").is_err());

        // 删除（活跃区）
        delete_agent(name).unwrap();
        assert!(!list_agents().unwrap().iter().any(|a| a.name == name));

        // 归档区删除
        archive_agent("arch-lifecycle-2").unwrap_or_else(|_| {
            create_agent("arch-lifecycle-2", "归档删除", None, None, None, None).unwrap();
            archive_agent("arch-lifecycle-2").unwrap();
        });
        // 上一步用 create+archive 保证该名字存在
        delete_archived_agent("arch-lifecycle-2").unwrap();
        assert!(
            !list_archived_agents()
                .unwrap()
                .iter()
                .any(|a| a.name == "arch-lifecycle-2")
        );

        // 删除不存在的 Agent → 报错
        assert!(delete_agent("no-such-agent").is_err());

        pipi_test_support::set_env_var("HOME", &prev);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn archive_scan_skips_dot_dir_with_manifest() {
        // 归档区里即使意外出现 agent.json，活跃列表也不得扫到（点目录硬跳过）
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-home-dot-{}", pipi_protocol::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let name = "dot-dir-agent";
        create_agent(name, "点目录防漏", None, None, None, None).unwrap();
        archive_agent(name).unwrap();
        // 往归档区里塞一个「坏掉」的 agent.json（不该影响活跃列表）
        let archive_agent_dir = agents_dir().unwrap().join(ARCHIVE_DIR).join(name);
        std::fs::write(archive_agent_dir.join("agent.json"), "{ broken json").unwrap();
        assert!(!list_agents().unwrap().iter().any(|a| a.name == name));

        pipi_test_support::set_env_var("HOME", &prev);
        let _ = std::fs::remove_dir_all(&home);
    }
}
