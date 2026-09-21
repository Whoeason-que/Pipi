//! Agent 组合最小闭环：结构化创建 → 独立运行 → 从 JSONL 重读输出。
//!
//! 本集成测试在自己的进程中改写 HOME，不接触用户真实的 ~/.pipi。模型端点
//! 指向本地 mock（必然拒绝连接或按剧本回放），不依赖外网。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use pipi_core::permissions::{BashPermissions, PermissionsConfig, SandboxMode};
use pipi_core::settings::{save_settings, ProviderConfig, Settings, Theme};
use pipi_core::tools::agent::{AgentRunStatus, CreateAgentTool, ReadAgentTool};
use pipi_core::tools::{AgentTool, ToolContext, ToolRegistry};
use pipi_core::types::{AbortSignal, Api, Model, ToolResultContent};
use serde_json::json;

/// 本文件内所有改写 HOME 的测试共用此锁：同一进程内并行测试会互相看到
/// 对方的 HOME，串行化才能保证隔离。
static HOME_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn create_run_and_read_agent_output() {
    let _home_guard = HOME_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pipi-agent-composition-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("HOME", &home);

    let model = Model {
        id: "composition-model".into(),
        name: "Composition Model".into(),
        api: Api::OpenAICompletions,
        base_url: "http://127.0.0.1:1/v1".into(),
        max_tokens: 512,
        context_window: 8192,
    };
    save_settings(&Settings {
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "local-refusing".into(),
            name: "Local Refusing Endpoint".into(),
            api: Api::OpenAICompletions,
            base_url: model.base_url.clone(),
            env_key: None,
            api_key: Some("test-key".into()),
        }],
        default_provider_id: None,
        compaction: Default::default(),
    })
    .unwrap();

    let permissions = PermissionsConfig {
        tools: vec![
            "read".into(),
            "glob".into(),
            "create_agent".into(),
            "run_agent".into(),
            "read_agent".into(),
        ],
        bash: BashPermissions::default(),
        sandbox: SandboxMode::WorkspaceWrite,
    };
    let context = ToolContext {
        workspace: std::fs::canonicalize(&workspace).unwrap(),
        memory_dir: None,
        read_roots: Vec::new(),
        permissions: Arc::new(permissions),
        sandbox: SandboxMode::WorkspaceWrite,
        resolved_env: Arc::new(BTreeMap::new()),
        abort: AbortSignal::new(),
        approver: None,
    };

    let create = CreateAgentTool::new(model);
    let created = create
        .execute(
            &context,
            &json!({
                "name": "composition-worker",
                "description": "A test worker",
                "instructions": "Return a concise result.",
                "tools": ["read"]
            }),
            &|_| {},
        )
        .await
        .unwrap();
    assert!(matches!(
        &created.content[0],
        ToolResultContent::Text { text } if text.contains("composition-worker")
    ));

    let definition = pipi_core::agents::load_agent("composition-worker").unwrap();
    assert_eq!(definition.permissions.tools, vec!["read"]);
    assert_eq!(definition.workspace.as_deref(), workspace.to_str());
    let instructions =
        std::fs::read_to_string(home.join(".pipi/agents/composition-worker/AGENTS.md")).unwrap();
    assert!(instructions.contains("Return a concise result."));

    // 基础 registry 有意忽略目标配置中的 Agent 组合工具；child 深度固定为 1。
    let child_registry = ToolRegistry::for_context(&context);
    assert!(child_registry.names().contains(&"read"));
    assert!(!child_registry.names().contains(&"run_agent"));

    let run = pipi_core::runtime::run_agent_once(
        "composition-worker",
        "Return a result",
        AbortSignal::new(),
    )
    .await
    .unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
    assert!(!run.response.is_empty());

    let read = ReadAgentTool
        .execute(
            &context,
            &json!({
                "name": "composition-worker",
                "sessionId": run.session_id
            }),
            &|_| {},
        )
        .await
        .unwrap();
    let details = read.details.unwrap();
    assert_eq!(details["agentName"], "composition-worker");
    assert_eq!(details["status"], "failed");
    assert_eq!(details["response"], run.response);

    // 即使目标 Agent 尚未配置模型，启动失败也必须落成可读取的 failed 输出，
    // 不能留下一个遮住旧结果的空 session。
    pipi_core::agents::create_agent(
        "unconfigured-worker",
        "A worker without a model",
        Some(workspace.to_str().unwrap()),
        Some(PermissionsConfig {
            tools: vec!["read".into()],
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        }),
        None,
        None,
    )
    .unwrap();
    let setup_failure = pipi_core::runtime::run_agent_once(
        "unconfigured-worker",
        "This input should remain readable",
        AbortSignal::new(),
    )
    .await
    .unwrap();
    assert_eq!(setup_failure.status, AgentRunStatus::Failed);
    assert!(setup_failure.response.contains("还未配置默认模型"));

    let reread_setup_failure = ReadAgentTool
        .execute(
            &context,
            &json!({
                "name": "unconfigured-worker",
                "sessionId": setup_failure.session_id
            }),
            &|_| {},
        )
        .await
        .unwrap();
    assert_eq!(reread_setup_failure.details.unwrap()["status"], "failed");

    let _ = std::fs::remove_dir_all(home);
}

// ============ 端到端：真实协议栈下父 Agent 调用子 Agent ============
//
// 本地 OpenAI 兼容 mock（rig 的 HTTP/SSE 栈原样参与），按请求序号回放：
//   req1: 父第 1 轮 → tool_call run_agent(mock-worker)
//   req2: 子第 1 轮 → 文本「子任务完成：42」
//   req3: 父第 2 轮 → 文本「最终：42」
// 验证 run_agent 工具把子输出作为 toolResult 交回父循环、子 JSONL 完整落盘、
// read_agent 能重读。

/// OpenAI 流式文本回合的 SSE 响应体。
fn sse_text(text: &str) -> String {
    let start = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let end = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

/// OpenAI 流式工具调用回合的 SSE 响应体（参数整体放在一个 chunk 里）。
fn sse_tool_call(name: &str, args: &serde_json::Value) -> String {
    let start = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{
            "index": 0, "id": "call-1", "type": "function",
            "function": {"name": name, "arguments": args.to_string()}
        }]}, "finish_reason": null}]
    });
    let end = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

fn http_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// 读取一个 HTTP 请求（丢弃内容，只保证响应前把 body 读完）。
fn drain_http_request(stream: &mut std::net::TcpStream) {
    use std::io::Read;
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) => {
                buffer.push(byte[0]);
                if buffer.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            _ => return,
        }
    }
    let headers = String::from_utf8_lossy(&buffer).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut remaining = vec![0u8; content_length];
    let _ = stream.read_exact(&mut remaining);
}

/// 按请求序号回放剧本的本地 mock；返回（地址，请求计数）。
fn spawn_scripted_mock(script: Vec<String>) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let script = Arc::new(script);
    let counter_for_thread = counter.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let script = script.clone();
            let counter = counter_for_thread.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                drain_http_request(&mut stream);
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let body = script
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| sse_text("(mock 剧本已耗尽)"));
                use std::io::Write;
                let _ = stream.write_all(http_response(&body).as_bytes());
            });
        }
    });
    (addr, counter)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_runs_child_agent_through_real_provider_stack() {
    let _home_guard = HOME_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pipi-agent-e2e-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("HOME", &home);

    let (addr, counter) = spawn_scripted_mock(vec![
        sse_tool_call("run_agent", &json!({"name": "mock-worker", "prompt": "计算 6x7"})),
        sse_text("子任务完成：42"),
        sse_text("最终：42"),
    ]);
    let base_url = format!("http://{addr}/v1");

    // context_window = 0：关闭摘要压缩（会替换内存历史），保持剧本请求计数精确
    let model = Model {
        id: "e2e-model".into(),
        name: "E2E Model".into(),
        api: Api::OpenAICompletions,
        base_url: base_url.clone(),
        max_tokens: 512,
        context_window: 0,
    };
    save_settings(&Settings {
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "e2e-mock".into(),
            name: "E2E Mock".into(),
            api: Api::OpenAICompletions,
            base_url: base_url.clone(),
            env_key: None,
            api_key: Some("test-key".into()),
        }],
        default_provider_id: None,
        compaction: Default::default(),
    })
    .unwrap();

    pipi_core::agents::create_agent(
        "mock-worker",
        "A scripted child",
        Some(workspace.to_str().unwrap()),
        Some(PermissionsConfig {
            tools: vec!["read".into()],
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        }),
        None,
        Some(model.clone()),
    )
    .unwrap();
    pipi_core::agents::create_agent(
        "mock-parent",
        "A scripted parent",
        Some(workspace.to_str().unwrap()),
        Some(PermissionsConfig {
            tools: vec!["run_agent".into(), "read_agent".into()],
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        }),
        None,
        Some(model),
    )
    .unwrap();

    let runtime = Arc::new(pipi_core::runtime::RuntimeState::new(
        tokio::runtime::Handle::current(),
    ));
    runtime
        .send_prompt("mock-parent", "让 worker 计算 6x7 并汇报", None, Arc::new(|_| {}))
        .unwrap();

    // 等待运行结束（上限 30 秒）
    for _ in 0..600 {
        if !runtime.session_running() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!runtime.session_running(), "运行应在超时前结束");

    let messages = runtime.session_messages().await.unwrap();
    let roles: Vec<&str> = messages.iter().map(|m| m.role()).collect();
    // user → assistant(run_agent) → toolResult → assistant(最终)
    assert_eq!(roles, vec!["user", "assistant", "toolResult", "assistant"]);

    let tool_result = messages
        .iter()
        .find_map(|m| match m {
            pipi_core::types::Message::ToolResult { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        ToolResultContent::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .expect("应有 toolResult");
    assert!(
        tool_result.contains("子任务完成：42"),
        "run_agent 的工具结果应包含子 Agent 输出: {tool_result}"
    );

    let final_text = messages
        .iter()
        .rev()
        .find_map(|m| match m {
            pipi_core::types::Message::Assistant { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        pipi_core::types::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .unwrap_or_default();
    assert!(final_text.contains("最终：42"), "父最终回复异常: {final_text}");

    // 恰好 3 次 LLM 调用：父两轮 + 子一轮
    assert_eq!(counter.load(Ordering::SeqCst), 3);

    // 子 Agent 的 JSONL 完整落盘，read_agent 可独立重读
    let child_output = pipi_core::tools::agent::read_agent_output("mock-worker", None).unwrap();
    assert_eq!(child_output.status, AgentRunStatus::Completed);
    assert!(child_output.response.contains("子任务完成：42"));

    let _ = std::fs::remove_dir_all(home);
}
