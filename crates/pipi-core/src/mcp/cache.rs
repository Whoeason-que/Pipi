//! `tools/list` 的磁盘快照。
//!
//! 目的只有一个：避免「重启后第一轮没有任何 MCP 工具」。连接与 `tools/list`
//! 是异步的，而注册表在同步段装配，所以第一轮只能读上一次的结果；指纹
//! （命令 / 参数 / env / 工作目录 / 沙箱）变了就视为失效，不猜。

use std::path::PathBuf;

use pipi_mcp::McpToolDef;
use pipi_venv::agents::agent_dir;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct CachedFile {
    fingerprint: String,
    #[serde(default)]
    tools: Vec<McpToolDef>,
}

/// 缓存目录：`<Agent 目录>/mcp-cache/<server>.json`。
///
/// 服务器名在 `validate_definition` 里已经限定为单段安全字符；这里再挡一次，
/// 因为磁盘上可能已有旧文件（手改 agent.json 不走校验）。
fn path_for(agent: &str, server: &str) -> Option<PathBuf> {
    if server.is_empty()
        || server.len() > 64
        || !server
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(
        agent_dir(agent)?
            .join("mcp-cache")
            .join(format!("{server}.json")),
    )
}

pub(super) fn load(agent: &str, server: &str, fingerprint: &str) -> Vec<McpToolDef> {
    let Some(path) = path_for(agent, server) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_str::<CachedFile>(&text) else {
        return Vec::new();
    };
    if file.fingerprint != fingerprint {
        return Vec::new();
    }
    file.tools
}

pub(super) fn store(agent: &str, server: &str, fingerprint: &str, tools: &[McpToolDef]) {
    let Some(path) = path_for(agent, server) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let file = CachedFile {
        fingerprint: fingerprint.to_string(),
        tools: tools.to_vec(),
    };
    let Ok(text) = serde_json::to_string_pretty(&file) else {
        return;
    };
    // 原子替换：并发会话各写各的服务器文件，读侧永远看到完整 JSON。
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_test_support::{HOME_LOCK, set_env_var};

    fn tool(name: &str) -> McpToolDef {
        McpToolDef {
            name: name.into(),
            title: None,
            description: Some("d".into()),
            input_schema: serde_json::json!({ "type": "object" }),
        }
    }

    #[test]
    fn cache_round_trips_only_with_matching_fingerprint() {
        let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = pipi_test_support::temp_dir("pipi-mcp-cache");
        set_env_var("HOME", &home);

        store("agent", "server", "fp-1", &[tool("a"), tool("b")]);
        let loaded = load("agent", "server", "fp-1");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "a");
        // 配置变了：缓存作废，不猜。
        assert!(load("agent", "server", "fp-2").is_empty());
        // 不安全的服务器名不落盘。
        store("agent", "../escape", "fp", &[tool("x")]);
        assert!(load("agent", "../escape", "fp").is_empty());
    }
}
