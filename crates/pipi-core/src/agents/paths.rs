use std::fs;
use std::path::PathBuf;

use pipi_protocol::session::ARCHIVE_DIR;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDefinition, list_agents, load_agent, save_agent};
    use pipi_test_support::HOME_LOCK;
    use pipi_tools::permissions::PermissionsConfig;

    #[test]
    fn agent_directory_rejects_path_traversal_and_separators() {
        assert!(agent_dir("safe-agent_1").is_some());
        for invalid in ["", ".", "..", "../escape", "nested/name", "nested\\name"] {
            assert!(
                agent_dir(invalid).is_none(),
                "accepted invalid agent name: {invalid}"
            );
        }
    }

    #[test]
    fn manifest_name_must_match_containing_agent_directory() {
        assert!(manifest_name_matches_directory("safe-agent", "safe-agent"));
        assert!(!manifest_name_matches_directory(
            "safe-agent",
            "other-agent"
        ));
        assert!(!manifest_name_matches_directory("../escape", "../escape"));
    }

    #[cfg(unix)]
    #[test]
    fn agent_directory_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-agent-link-home-{}", pipi_protocol::new_id()));
        let external = std::env::temp_dir().join(format!(
            "pipi-agent-link-external-{}",
            pipi_protocol::new_id()
        ));
        let linked = home.join(".pipi/agents/linked-agent");
        std::fs::create_dir_all(home.join(".pipi/agents")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(&external, &linked).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);
        let def = AgentDefinition {
            name: "linked-agent".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: None,
            permissions: PermissionsConfig::default(),
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };

        assert!(agent_dir("linked-agent").is_none());
        assert!(load_agent("linked-agent").is_err());
        assert!(save_agent(&def).is_err());
        assert!(ensure_agent_dir("linked-agent").is_err());
        assert!(list_agents().unwrap().is_empty());

        pipi_test_support::set_env_var("HOME", previous_home);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn agent_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!(
            "pipi-agent-root-link-home-{}",
            pipi_protocol::new_id()
        ));
        let external = std::env::temp_dir().join(format!(
            "pipi-agent-root-link-external-{}",
            pipi_protocol::new_id()
        ));
        std::fs::create_dir_all(home.join(".pipi")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(&external, home.join(".pipi/agents")).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);
        assert!(list_agents().is_err());
        assert!(ensure_agent_dir("root-link-agent").is_err());
        pipi_test_support::set_env_var("HOME", previous_home);

        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();
    }

    #[test]
    fn tilde_expansion() {
        // 读取 HOME 前先拿锁，避免与其他测试的 set_var 竞争
        let _guard = HOME_LOCK.lock().unwrap();
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(expand_tilde("~/foo"), home.join("foo").to_string_lossy());
        assert_eq!(expand_tilde("~"), home.to_string_lossy());
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("rel/path"), "rel/path");
    }
}
