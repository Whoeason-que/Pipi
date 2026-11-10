//! 宿主侧 MCP 端到端：agent.json 声明 → 后台连接 → 工具声明 → 调用 →
//! `list_changed` 刷新 → 重启复用。
//!
//! 用 `sh` 写一个最小的 MCP 服务器（不需要 Python/Node），只覆盖 host 需要的那部分：
//! initialize / tools/list / tools/call / list_changed。沙箱用 danger-full-access：
//! 这里测的是装配与协议，沙箱本身由 pipi-sandbox 的测试负责。

#![cfg(unix)]
#![allow(clippy::await_holding_lock)] // HOME_LOCK 必须覆盖整个异步测试

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use pipi_core::mcp::{McpManager, McpTool, McpToolSpec};
use pipi_protocol::AbortSignal;
use pipi_protocol::ToolResultContent;
use pipi_protocol::permissions::{PermissionsConfig, SandboxMode};
use pipi_protocol::tools::{AgentTool, ToolContext};
use pipi_test_support::{HOME_LOCK, remove_env_var, set_env_var, temp_dir};
use pipi_venv::agents::AgentDefinition;

const AGENT: &str = "mcp-e2e";

const SERVER: &str = r#"
grew=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"sh-mock","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      if [ "$grew" = "1" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo a message","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}},{"name":"grow","description":"Announce a changed tool list","inputSchema":{"type":"object","properties":{}}},{"name":"echo2","description":"Appears after grow","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo a message","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}},{"name":"grow","description":"Announce a changed tool list","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      fi
      ;;
    *'"method":"tools/call"'*)
      name=$(printf '%s' "$line" | sed -E 's/.*"name":"([^"]*)".*/\1/')
      if [ "$name" = "grow" ]; then
        grew=1
        printf '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n'
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"grown"}]}}\n' "$id"
      else
        message=$(printf '%s' "$line" | sed -E 's/.*"message":"([^"]*)".*/\1/')
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo: %s"}]}}\n' "$id" "$message"
      fi
      ;;
  esac
done
"#;

fn definition(workspace: &Path) -> AgentDefinition {
    serde_json::from_value(serde_json::json!({
        "name": AGENT,
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

/// HOME 必须在本测试期间保持为临时目录（缓存写到 `<Agent>/mcp-cache/`），
/// 所以隔离与恢复放在同一个 fixture 里；`_lock` 在大批字段之后析构。
struct Fixture {
    previous_home: Option<OsString>,
    _home: PathBuf,
    workspace: PathBuf,
    def: AgentDefinition,
    env: BTreeMap<String, String>,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_home = std::env::var_os("HOME");
        let home = temp_dir(prefix);
        set_env_var("HOME", &home);
        let workspace = home.join("ws");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let env = environment();
        let def = definition(&workspace);
        Self {
            previous_home,
            _home: home,
            workspace,
            def,
            env,
            _lock: lock,
        }
    }

    fn context(&self) -> ToolContext {
        ToolContext {
            workspace: self.workspace.clone(),
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox: SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(self.env.clone()),
            abort: AbortSignal::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        match self.previous_home.take() {
            Some(value) => set_env_var("HOME", value),
            None => remove_env_var("HOME"),
        }
    }
}

async fn wait_for_tools(manager: &McpManager, count: usize) -> Vec<McpToolSpec> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let specs = manager.tools_for(AGENT);
        if specs.len() == count {
            return specs;
        }
        if Instant::now() > deadline {
            panic!(
                "expected {count} tools, got {}: {:?}",
                specs.len(),
                manager.status(AGENT)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn spec(specs: &[McpToolSpec], tool: &str) -> McpToolSpec {
    specs
        .iter()
        .find(|spec| spec.tool.name == tool)
        .unwrap_or_else(|| panic!("tool {tool} is not declared"))
        .clone()
}

#[tokio::test]
async fn declared_server_becomes_a_callable_tool_and_survives_a_restart() {
    let fixture = Fixture::new("pipi-mcp-host");
    let manager = McpManager::new(tokio::runtime::Handle::current());
    manager.prepare(&fixture.def, &fixture.env);
    let specs = wait_for_tools(&manager, 2).await;
    assert!(
        specs
            .iter()
            .any(|spec| spec.wire_name == "mcp__sh-mock__echo")
    );
    let status = manager.status(AGENT);
    assert_eq!(status.len(), 1);
    assert!(status[0].connected, "status: {status:?}");
    assert_eq!(status[0].tools, 2);

    let tool = McpTool::new(manager.clone(), AGENT.into(), spec(&specs, "echo"));
    let output = tool
        .execute(
            &fixture.context(),
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
    restarted.prepare(&fixture.def, &fixture.env);
    assert_eq!(restarted.tools_for(AGENT).len(), 2);
    restarted.shutdown_all().await;
    manager.shutdown_all().await;
}

#[tokio::test]
async fn tools_list_changed_refreshes_the_catalog_and_the_snapshot() {
    let fixture = Fixture::new("pipi-mcp-grow");
    let manager = McpManager::new(tokio::runtime::Handle::current());
    manager.prepare(&fixture.def, &fixture.env);
    let specs = wait_for_tools(&manager, 2).await;
    assert!(specs.iter().all(|spec| spec.tool.name != "echo2"));

    let tool = McpTool::new(manager.clone(), AGENT.into(), spec(&specs, "grow"));
    tool.execute(&fixture.context(), &serde_json::json!({}), &|_| {})
        .await
        .expect("grow should succeed");

    let specs = wait_for_tools(&manager, 3).await;
    assert!(specs.iter().any(|spec| spec.tool.name == "echo2"));
    assert_eq!(manager.status(AGENT)[0].tools, 3);

    // 快照同步更新：新的 manager 不必等连接就能看到 3 个工具。
    let restarted = McpManager::new(tokio::runtime::Handle::current());
    restarted.prepare(&fixture.def, &fixture.env);
    assert_eq!(restarted.tools_for(AGENT).len(), 3);
    restarted.shutdown_all().await;
    manager.shutdown_all().await;
}

#[tokio::test]
async fn disconnected_call_is_not_replayed_after_an_external_side_effect() {
    let mut fixture = Fixture::new("pipi-mcp-no-replay");
    let marker = fixture.workspace.join("side-effects");
    fixture
        .env
        .insert("MARKER".into(), marker.to_string_lossy().into_owned());
    fixture.def.mcp_servers[0].args = vec!["-c".into(), r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"test","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"mutate","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*) printf x >> "$MARKER"; exit 0 ;;
  esac
done
"#.into()];
    let manager = McpManager::new(tokio::runtime::Handle::current());
    manager.prepare(&fixture.def, &fixture.env);
    wait_for_tools(&manager, 1).await;
    let error = manager
        .call(
            AGENT,
            "sh-mock",
            "mutate",
            serde_json::json!({}),
            &AbortSignal::new(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("outcome is unknown"), "{error}");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "x");
    manager.shutdown_all().await;
}

#[tokio::test]
async fn cancellation_interrupts_waiting_for_the_server_serial_lock() {
    let mut fixture = Fixture::new("pipi-mcp-lock-cancel");
    fixture.def.mcp_servers[0].args = vec![
        "-c".into(),
        SERVER.replace("name=$(printf", "sleep 2\n      name=$(printf"),
    ];
    let manager = McpManager::new(tokio::runtime::Handle::current());
    manager.prepare(&fixture.def, &fixture.env);
    wait_for_tools(&manager, 2).await;
    let first_manager = manager.clone();
    let first = tokio::spawn(async move {
        first_manager
            .call(
                AGENT,
                "sh-mock",
                "echo",
                serde_json::json!({"message":"one"}),
                &AbortSignal::new(),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let abort = AbortSignal::new();
    let cancelling = abort.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancelling.abort();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        manager.call(
            AGENT,
            "sh-mock",
            "echo",
            serde_json::json!({"message":"two"}),
            &abort,
        ),
    )
    .await
    .expect("lock wait should be cancellable");
    assert!(result.unwrap_err().contains("before MCP call was sent"));
    first.await.unwrap().unwrap();
    manager.shutdown_all().await;
}
