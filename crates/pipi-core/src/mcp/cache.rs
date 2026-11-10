//! `tools/list` 的磁盘快照。
//!
//! 目的只有一个：避免「重启后第一轮没有任何 MCP 工具」。连接与 `tools/list`
//! 是异步的，而注册表在同步段装配，所以第一轮只能读上一次的结果；指纹
//! （命令 / 参数 / env / 工作目录 / 沙箱）变了就视为失效，不猜。

use std::io::Write;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

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

/// A per-Agent key prevents an exported cache from enabling offline guesses of
/// low-entropy environment values. A missing/unusable key makes the cache miss.
fn cache_key(agent: &str, create: bool) -> Option<Vec<u8>> {
    let dir = agent_dir(agent)?.join("mcp-cache");
    if create {
        std::fs::create_dir_all(&dir).ok()?;
    }
    if !pipi_protocol::fs::ensure_real_directory(&dir, "MCP cache directory").ok()? {
        return None;
    }
    let path = dir.join(".fingerprint-key");
    if create && !path.exists() {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes).ok()?;
                file.sync_all().ok()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    if !pipi_protocol::fs::ensure_real_file(&path, "MCP cache key").ok()? {
        return None;
    }
    let key = std::fs::read(path).ok()?;
    (key.len() == 32).then_some(key)
}

fn keyed_fingerprint(key: &[u8], fingerprint: &str) -> String {
    // HMAC-SHA256 with a 32-byte randomly generated key.
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for (i, byte) in key.iter().enumerate() {
        inner_pad[i] ^= byte;
        outer_pad[i] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(fingerprint.as_bytes());
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    let digest = outer.finalize();
    format!(
        "v3:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// Remove old cache files containing plaintext fingerprints, including orphaned
/// servers and interrupted legacy .tmp writes. Unrecognized files are untouched.
pub(super) fn purge_legacy(agent: &str) {
    let Some(dir) = agent_dir(agent).map(|path| path.join("mcp-cache")) else {
        return;
    };
    if !pipi_protocol::fs::ensure_real_directory(&dir, "MCP cache directory").unwrap_or(false) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !(name.ends_with(".json") || name.ends_with(".json.tmp")) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        // The old implementation used this fixed temporary suffix and could
        // leave a partially written JSON containing secrets after a crash.
        if name.ends_with(".json.tmp") {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if metadata.len() > 16 * 1024 * 1024 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = serde_json::from_str::<CachedFile>(&text) else {
            continue;
        };
        if !file.fingerprint.starts_with("v2:") && !file.fingerprint.starts_with("v3:") {
            let _ = std::fs::remove_file(path);
        }
    }
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
    // Old fingerprints contained the expanded environment and arguments in
    // plaintext. Delete this disposable cache on first access, never reuse it.
    if !file.fingerprint.starts_with("v2:") && !file.fingerprint.starts_with("v3:") {
        let _ = std::fs::remove_file(&path);
        return Vec::new();
    }
    let Some(key) = cache_key(agent, false) else {
        return Vec::new();
    };
    if file.fingerprint != keyed_fingerprint(&key, fingerprint) {
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
    let Some(key) = cache_key(agent, true) else {
        return;
    };
    let file = CachedFile {
        fingerprint: keyed_fingerprint(&key, fingerprint),
        tools: tools.to_vec(),
    };
    let Ok(text) = serde_json::to_string_pretty(&file) else {
        return;
    };
    // Separate temporary files avoid concurrent writers sharing a .tmp path;
    // private permissions also protect server-supplied schema and descriptions.
    let _ = pipi_protocol::fs::write_private_atomic(&path, text);
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
    fn keyed_fingerprint_matches_hmac_sha256_reference() {
        let key = vec![0x0b; 20];
        assert_eq!(
            keyed_fingerprint(&key, "Hi There"),
            "v3:b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn cache_round_trips_only_with_matching_fingerprint() {
        let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = pipi_test_support::temp_dir("pipi-mcp-cache");
        set_env_var("HOME", &home);

        store("agent", "server", "v2:fp-1", &[tool("a"), tool("b")]);
        let loaded = load("agent", "server", "v2:fp-1");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "a");
        // 配置变了：缓存作废，不猜。
        assert!(load("agent", "server", "v2:fp-2").is_empty());
        // Legacy cache embedded expanded secrets. Discard it instead of loading it.
        let path = path_for("agent", "server").unwrap();
        std::fs::write(
            &path,
            r#"{"fingerprint":"TOKEN=synthetic-secret","tools":[]}"#,
        )
        .unwrap();
        assert!(load("agent", "server", "v2:fp-1").is_empty());
        assert!(!path.exists());
        let orphan = path_for("agent", "removed-server").unwrap();
        let old_tmp = orphan.with_extension("json.tmp");
        std::fs::write(
            &orphan,
            r#"{"fingerprint":"TOKEN=synthetic-secret","tools":[]}"#,
        )
        .unwrap();
        std::fs::write(&old_tmp, r#"{"fingerprint":"TOKEN=synthetic-secret"#).unwrap();
        purge_legacy("agent");
        assert!(!orphan.exists());
        assert!(!old_tmp.exists());
        // 不安全的服务器名不落盘。
        store("agent", "../escape", "fp", &[tool("x")]);
        assert!(load("agent", "../escape", "fp").is_empty());
    }
}
