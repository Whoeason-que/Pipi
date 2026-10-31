use std::path::PathBuf;

use pipi_protocol::fs::ensure_real_directory;
use pipi_protocol::session::ARCHIVE_DIR;

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
    let agents = crate::av::paths::agents_root().ok_or_else(|| "无法定位用户主目录".to_string())?;
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

/// 会话 notebook 文件：`<Agent 目录>/notebooks/<session_id>.md`。
///
/// 会话 id 直接拼进文件名，所以必须当作用户输入来校验：只接受
/// ASCII 字母/数字/`-`/`_` 的单段名称。历史 JSONL 导入的会话 id 若含其它
/// 字符，`None` 表示「这条会话没有 notebook」，而不是回退到不安全路径。
pub fn notebook_path(name: &str, session_id: &str) -> Option<PathBuf> {
    if !is_safe_file_stem(session_id) {
        return None;
    }
    agent_dir(name).map(|dir| dir.join("notebooks").join(format!("{session_id}.md")))
}

/// 单段文件名校验（会话 id 用；Agent 名另有 `validate_agent_name`）。
fn is_safe_file_stem(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 会话换 id 时把 notebook 带到新会话（压缩分叉 / 手动分叉）。
///
/// notebook 是这次工作的计划与走查，换了会话 id 就该跟着走，而不是在右栏
/// 里凭空消失。目标已存在时不覆盖；源不存在时什么也不做（没有 notebook 是常态）。
pub fn carry_notebook(name: &str, from_session: &str, to_session: &str) -> Result<(), String> {
    let (Some(from), Some(to)) = (
        notebook_path(name, from_session),
        notebook_path(name, to_session),
    ) else {
        return Ok(());
    };
    if !from.is_file() || to.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create notebook directory: {e}"))?;
    }
    std::fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|e| format!("Failed to copy notebook: {e}"))
}

/// 展开 `~` 前缀（仅支持 `~` 与 `~/...`；规则复用 av 的同一份实现）。
pub fn expand_tilde(path: &str) -> String {
    crate::av::discovery::expand_tilde(path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDefinition, list_agents, load_agent, save_agent};
    use pipi_protocol::permissions::PermissionsConfig;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn agent_directory_rejects_path_traversal_and_separators() {
        // 读 HOME 派生的路径前先拿锁：同进程内还有别的用例在改 HOME / 把
        // `~/.pipi/agents` 换成符号链接，不持锁就会读到中途状态。
        let _guard = HOME_LOCK.lock().unwrap();
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
            system_prompt: Default::default(),
        };

        assert!(agent_dir("linked-agent").is_none());
        assert!(load_agent("linked-agent").is_err());
        assert!(save_agent(&def).is_err());
        assert!(checked_agent_dir("linked-agent").is_err());
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
        assert!(checked_agent_dir("root-link-agent").is_err());
        pipi_test_support::set_env_var("HOME", previous_home);

        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();
    }

    #[test]
    fn notebook_path_is_session_scoped_and_single_segment() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-notebook-home-{}", pipi_protocol::new_id()));
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let path = notebook_path("agent-a", "1730000000000-abc123").expect("valid session id");
        assert_eq!(
            path,
            home.join(".pipi/agents/agent-a/notebooks/1730000000000-abc123.md")
        );
        for invalid in [
            "",
            ".",
            "..",
            "../escape",
            "nested/name",
            "nested\\name",
            ".hidden",
            "id with space",
        ] {
            assert!(
                notebook_path("agent-a", invalid).is_none(),
                "accepted session id: {invalid}"
            );
        }

        // 分叉时 notebook 跟着会话 id 走；目标已有内容时不覆盖。
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "## Plan\n").unwrap();
        carry_notebook("agent-a", "1730000000000-abc123", "1730000000000-def456").unwrap();
        let forked = notebook_path("agent-a", "1730000000000-def456").unwrap();
        assert_eq!(std::fs::read_to_string(&forked).unwrap(), "## Plan\n");
        std::fs::write(&forked, "## Plan\n\nnewer\n").unwrap();
        carry_notebook("agent-a", "1730000000000-abc123", "1730000000000-def456").unwrap();
        assert_eq!(
            std::fs::read_to_string(&forked).unwrap(),
            "## Plan\n\nnewer\n",
            "目标已存在时不应被源覆盖"
        );

        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = std::fs::remove_dir_all(&home);
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
