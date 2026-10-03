use super::compaction::persist_compaction;
use super::*;

use std::future::pending;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn compaction_summary_request_uses_active_session_id() {
    let options = compaction_stream_options("test-key", "active-session");
    assert_eq!(options.session_id.as_deref(), Some("active-session"));
    assert_eq!(options.api_key.as_deref(), Some("test-key"));
}

#[test]
fn temporary_compaction_updates_memory_ledger_without_forking() {
    let mut ledger = SessionWriter::temporary();
    let keep_from = ledger
        .append_message(&Message::user_text("保留这条"))
        .unwrap();
    ledger
        .append_message(&Message::assistant_text("回答", "mock"))
        .unwrap();
    let source_tip = ledger.tip_id().unwrap().to_string();
    let writer = Arc::new(Mutex::new(ledger));
    let record = pipi_core::session::CompactionRecord {
        summary: "内存摘要",
        strategy: "summary",
        keep_from_entry: &keep_from,
        source_tip: &source_tip,
        usage: None,
    };

    let switched = persist_compaction(
        &writer,
        None,
        &record,
        pipi_core::settings::CompactionSettings {
            fork_before_compact: true,
            archive_original: true,
        },
    )
    .expect("临时账本压缩");

    assert!(switched.is_none(), "临时压缩不能切换或分叉会话");
    let ledger = writer.lock().unwrap();
    assert!(ledger.is_temporary());
    assert!(ledger.persistent_path().is_none());
    assert_ne!(ledger.tip_id(), Some(source_tip.as_str()));
}

#[test]
fn overlapping_generations_keep_new_run_visible() {
    let state = Arc::new(RunState::new());
    let old_guard = RunningGuard::new(state.clone());
    let new_guard = RunningGuard::new(state.clone());

    drop(old_guard);
    assert!(state.is_running());

    drop(new_guard);
    assert!(!state.is_running());
}

#[test]
fn tokio_abort_drops_running_guard() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let state = Arc::new(RunState::new());
    let task_state = state.clone();

    runtime.block_on(async move {
        let handle = tokio::spawn(async move {
            let _guard = RunningGuard::new(task_state);
            pending::<()>().await;
        });
        tokio::task::yield_now().await;
        assert!(state.is_running());

        handle.abort();
        assert!(handle.await.is_err());
        assert!(!state.is_running());
    });
}

#[test]
fn tokio_panic_drops_running_guard() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let state = Arc::new(RunState::new());
    let task_state = state.clone();

    runtime.block_on(async move {
        let handle = tokio::spawn(async move {
            let _guard = RunningGuard::new(task_state);
            panic!("test panic");
        });

        assert!(handle.await.is_err());
        assert!(!state.is_running());
    });
}

#[test]
fn running_state_is_visible_across_threads() {
    let state = Arc::new(RunState::new());
    let started = Arc::new(Barrier::new(2));
    let observed = Arc::new(Barrier::new(2));
    let finished = Arc::new(Barrier::new(2));
    let observer_state = state.clone();
    let observer_started = started.clone();
    let observer_observed = observed.clone();
    let observer_finished = finished.clone();

    let observer = thread::spawn(move || {
        observer_started.wait();
        assert!(observer_state.is_running());
        observer_observed.wait();
        observer_finished.wait();
        assert!(!observer_state.is_running());
    });

    let guard = RunningGuard::new(state.clone());
    started.wait();
    observed.wait();
    drop(guard);
    finished.wait();
    observer.join().unwrap();
}

/// 回归：桌面壳的 Tauri command 跑在 GTK 主线程上，那里没有 reactor。
/// 核心必须把运行 spawn 到注入的运行时上，而不是依赖调用线程的上下文 ——
/// 修复前这里会 panic（there is no reactor running），且 panic 发生在主线程、
/// 直接中止整个进程。
#[test]
fn spawn_run_works_without_ambient_reactor() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let state = RuntimeState::new(runtime.handle().clone());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = done.clone();

    // 关键：不套 runtime.block_on —— 模拟「调用线程不在运行时里」
    state.spawn_run(async move {
        flag.store(true, Ordering::SeqCst);
    });
    assert!(!done.load(Ordering::SeqCst), "任务不该在调用线程上同步执行");

    runtime.block_on(async {
        tokio::task::yield_now().await;
    });
    assert!(
        done.load(Ordering::SeqCst),
        "任务必须落在注入的运行时上执行"
    );
}

#[test]
fn resolve_model_prefers_session_model_over_agent_default() {
    // 该测试依赖「HOME 下没有 settings.json → 默认 provider 表」，
    // 且 HOME 是进程级状态：持全局锁 + 指向空目录，保证确定性。
    let _guard = crate::HOME_LOCK.lock().unwrap();
    let previous_home = std::env::var("HOME").unwrap_or_default();
    let empty_home = std::env::temp_dir().join(format!(
        "pipi-resolve-model-home-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    std::fs::create_dir_all(empty_home.join(".pipi")).unwrap();
    crate::set_env_var("HOME", &empty_home);

    let default_model = Model {
        id: "gpt-4o".into(),
        name: "GPT-4o".into(),
        api: pipi_protocol::Api::OpenAICompletions,
        base_url: "https://api.openai.com/v1".into(),
        max_tokens: 4096,
        context_window: 128000,
    };
    let session_model = Model {
        id: "claude-sonnet-4-5".into(),
        name: "Claude Sonnet".into(),
        api: pipi_protocol::Api::AnthropicMessages,
        base_url: "https://api.anthropic.com".into(),
        max_tokens: 8192,
        context_window: 200000,
    };

    let def = AgentDefinition {
        name: "test-agent".into(),
        description: String::new(),
        model: "gpt-4o".into(),
        provider: Some(default_model.clone()),
        workspace: None,
        permissions: Default::default(),
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    // 未配置 key 时的校验
    crate::remove_env_var("ANTHROPIC_API_KEY");
    crate::remove_env_var("OPENAI_API_KEY");

    // 回退默认模型
    let err = super::resolve_model(&def, None, None).unwrap_err();
    assert!(err.contains("未配置 API 密钥"));

    // 优先会话覆盖模型
    let err = super::resolve_model(&def, Some(&session_model), None).unwrap_err();
    assert!(err.contains("api.anthropic.com"));

    // 配上 key 后成功解析
    crate::set_env_var("ANTHROPIC_API_KEY", "test-key");
    let (resolved, key) = super::resolve_model(&def, Some(&session_model), None).unwrap();
    assert_eq!(resolved.id, "claude-sonnet-4-5");
    assert_eq!(key, "test-key");
    crate::remove_env_var("ANTHROPIC_API_KEY");
    crate::set_env_var("HOME", &previous_home);
    let _ = std::fs::remove_dir_all(&empty_home);
}

#[cfg(test)]
mod child_timeout_tests {
    use super::*;

    #[test]
    fn timeout_terminal_repairs_a_persisted_tool_call_before_appending_error() {
        let sessions = std::env::temp_dir().join(format!(
            "pipi-timeout-pairing-{}-{}",
            std::process::id(),
            pipi_core::session::new_id()
        ));
        std::fs::create_dir_all(&sessions).unwrap();
        let writer = Arc::new(Mutex::new(SessionWriter::create(&sessions).unwrap()));
        let path = writer.lock().unwrap().path().to_path_buf();

        let user = Message::user_text("run a tool");
        let tool_call = Message::Assistant {
            content: vec![pipi_protocol::ContentBlock::ToolCall {
                id: "call-timeout".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "sleep 30" }),
            }],
            api: "openai-completions".into(),
            provider: "test".into(),
            model: "test-model".into(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: pipi_protocol::now_millis(),
            duration_ms: None,
        };
        {
            let mut guard = writer.lock().unwrap();
            guard.append_message(&user).unwrap();
            guard.append_message(&tool_call).unwrap();
        }

        let model = Model {
            id: "test-model".into(),
            name: "Test model".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: String::new(),
            max_tokens: 1,
            context_window: 0,
        };
        append_child_timeout_terminal(&writer, &model, std::time::Duration::from_secs(1)).unwrap();

        let entries = load_session(&path).unwrap();
        let messages =
            pipi_core::session::rebuild_messages(&pipi_core::session::active_path(&entries));
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2],
            Message::ToolResult {
                tool_call_id,
                tool_name,
                is_error: true,
                ..
            } if tool_call_id == "call-timeout" && tool_name == "bash"
        ));
        assert!(matches!(
            &messages[3],
            Message::Assistant {
                stop_reason: StopReason::Error,
                error_message: Some(message),
                ..
            } if message.contains("超时")
        ));

        drop(writer);
        std::fs::remove_dir_all(sessions).unwrap();
    }

    /// 子 Agent 挂死（端点接受连接但不回包）时，运行时限必须终止本轮并把
    /// 超时终态落盘 —— read_agent 读到 Failed 而不是永远的 pending。
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn child_run_times_out_and_persists_terminal_state() {
        let _guard = crate::HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            // 故意持有每条连接：请求悬挂，直到 tokio 超时触发
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });

        let home = std::env::temp_dir().join(format!(
            "pipi-child-timeout-{}-{}",
            std::process::id(),
            pipi_core::session::new_id()
        ));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        crate::set_env_var("HOME", &home);

        let base_url = format!("http://{addr}/v1");
        pipi_core::settings::save_settings(&pipi_core::settings::Settings {
            theme: pipi_core::settings::Theme::Dark,
            providers: vec![pipi_core::settings::ProviderConfig {
                id: "silent".into(),
                name: "Silent".into(),
                api: pipi_protocol::Api::OpenAICompletions,
                base_url: base_url.clone(),
                env_key: None,
                api_key: Some("test-key".into()),
            }],
            default_provider_id: None,
            compaction: Default::default(),
            retry: Default::default(),
        })
        .unwrap();

        agents::create_agent(
            "timeout-worker",
            "A worker whose endpoint never responds",
            Some(workspace.to_str().unwrap()),
            Some(pipi_tools::permissions::PermissionsConfig {
                tools: vec!["read".into()],
                bash: Default::default(),
                sandbox: pipi_tools::permissions::SandboxMode::WorkspaceWrite,
            }),
            None,
            Some(pipi_protocol::Model {
                id: "silent-model".into(),
                name: "Silent Model".into(),
                api: pipi_protocol::Api::OpenAICompletions,
                base_url,
                max_tokens: 64,
                context_window: 4096,
            }),
        )
        .unwrap();

        let result = run_agent_once_inner(
            "timeout-worker",
            "hang",
            AbortSignal::new(),
            std::time::Duration::from_millis(300),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.status, AgentRunStatus::Failed);
        assert!(
            result.response.contains("超时"),
            "应报超时: {}",
            result.response
        );

        // 终态已落盘：read_agent 读到 Failed 而不是 pending
        let reread =
            pipi_core::tools::agent::read_agent_output("timeout-worker", Some(&result.session_id))
                .unwrap();
        assert_eq!(reread.status, AgentRunStatus::Failed);

        let _ = std::fs::remove_dir_all(home);
    }
}
