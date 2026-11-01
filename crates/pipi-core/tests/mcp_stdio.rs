//! 宿主侧 MCP 端到端：agent.json 声明 → 后台连接 → 工具声明 → 调用 → 重启复用。
//!
//! 用 `sh` 写一个最小的 MCP 服务器（不需要 Python/Node），只覆盖 host 需要的那部分：
//! initialize / tools/list / tools/call。沙箱用 danger-full-access：这里测的是装配
//! 与协议，沙箱本身由 pipi-sandbox 的测试负责。

#![cfg(unix)]
#![allow(clippy::await_holding_lock)] // HOME_LOCK 必须覆盖整个异步测试

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipi_core::mcp::{McpManager, McpTool, McpToolSpec};
use pipi_protocol::AbortSignal;
use pipi_protocol::ToolResultContent;
use pipi_protocol::permissions::{PermissionsConfig, SandboxMode};
use pipi_protocol::tools::{AgentTool, ToolContext};
use pipi_test_support::{HOME_LOCK, remove_env_var, set_env_var, temp_dir};
use pipi_venv::agents::AgentDefinition;

const SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"sh-mock","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo a message","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      message=$(printf '%s' "$line" | sed -E 's/.*"message":"([^"]*)".*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo: %s"}]}}\n' "$id" "$message"
      ;;
  esac
done
"#;

fn definition(workspace: &Path) -> AgentDefinition {
    serde_json::from_value(serde_json::json!({
        "name": "mcp-e2e",
        "workspace": workspace.display().to_string(),
        "permissions": { "tools": ["read"], "sandbox": "danger-full-access" },
        "mcpServers": [{
            "name": "sh-mock",
            "command": "sh",
            "args": ["-c", SERVER],
            "enabled": true,
        }],
    }))
    .expect("test definition is valid")
}

fn environment() -> BTreeMap<String, String> {
    [(
        "PATH".to_string(),
        "/usr/bin:/bin:/usr/local/bin".to_string(),
    )]
    .into_iter()
    .collect()
}

fn context(workspace: &Path, env: &BTreeMap<String, String>) -> ToolContext {
    ToolContext {
        workspace: workspace.to_path_buf(),
        memory_dir: None,
        notebook_path: None,
        read_roots: Vec::new(),
        permissions: Arc::new(PermissionsConfig::default()),
        sandbox: SandboxMode::DangerFullAccess,
        resolved_env: Arc::new(env.clone()),
        abort: AbortSignal::new(),
    }
}

async fn wait_for_tool(manager: &McpManager) -> McpToolSpec {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(spec) = manager.tools_for("mcp-e2e").into_iter().next() {
            return spec;
        }
        if Instant::now() > deadline {
            panic!(
                "server did not connect in time: {:?}",
                manager.status("mcp-e2e")
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn declared_server_becomes_a_callable_tool_and_survives_a_restart() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_home = std::env::var_os("HOME");
    let home = temp_dir("pipi-mcp-host");
    set_env_var("HOME", &home);
    let workspace = home.join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let env = environment();
    let def = definition(&workspace);

    let manager = McpManager::new(tokio::runtime::Handle::current());
    manager.prepare(&def, &env);
    let spec = wait_for_tool(&manager).await;
    assert_eq!(spec.wire_name, "mcp__sh-mock__echo");
    let status = manager.status("mcp-e2e");
    assert_eq!(status.len(), 1);
    assert!(status[0].connected, "status: {status:?}");
    assert_eq!(status[0].tools, 1);

    let tool = McpTool::new(manager.clone(), "mcp-e2e".into(), spec);
    let output = tool
        .execute(
            &context(&workspace, &env),
            &serde_json::json!({ "message": "hi" }),
            &|_| {},
        )
        .await
        .expect("tool call should succeed");
    assert!(
        matches!(&output.content[0], ToolResultContent::Text { text } if text == "echo: hi"),
        "unexpected output: {:?}",
        output.content
    );

    // 模拟重启：新 manager 还没连上，但磁盘快照让第一轮就能声明工具。
    let restarted = McpManager::new(tokio::runtime::Handle::current());
    restarted.prepare(&def, &env);
    assert_eq!(restarted.tools_for("mcp-e2e").len(), 1);
    restarted.shutdown_all().await;
    manager.shutdown_all().await;

    match previous_home {
        Some(value) => set_env_var("HOME", value),
        None => remove_env_var("HOME"),
    }
}
