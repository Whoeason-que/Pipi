use super::*;

fn owner() -> BackgroundTaskOwner {
    BackgroundTaskOwner::new("agent", "session", 1)
}

fn entry(status: BackgroundTaskStatus) -> JobEntry {
    let (control, _rx) = mpsc::unbounded_channel();
    JobEntry {
        owner: Mutex::new(owner()),
        pid: AtomicU32::new(0),
        state: Mutex::new(JobState {
            info: BackgroundTaskInfo {
                job_id: "job-1".into(),
                kind: BackgroundTaskKind::Shell,
                agent_name: "agent".into(),
                session_id: "session".into(),
                run_id: 1,
                command: "echo".into(),
                cwd: "/tmp".into(),
                target_agent: None,
                child_session_id: None,
                status,
                started_at: 1,
                completed_at: None,
                exit_code: None,
                output_cursor: 0,
                output_truncated: false,
                error: None,
                result: None,
            },
            output: VecDeque::new(),
            output_bytes: 0,
            next_seq: 0,
        }),
        control,
        notify: Notify::new(),
        metadata_path: PathBuf::new(),
        log_path: PathBuf::new(),
    }
}

#[test]
fn output_buffer_is_bounded_and_keeps_cursor() {
    let entry = entry(BackgroundTaskStatus::Running);
    for _ in 0..1000 {
        entry.append_output("stdout", "x".repeat(1024)).unwrap();
    }
    let snapshot = entry.snapshot(0).unwrap();
    assert_eq!(snapshot.next_seq, 1000);
    assert!(snapshot.task.output_truncated);
    assert!(
        snapshot
            .output
            .iter()
            .map(|item| item.text.len())
            .sum::<usize>()
            <= MAX_OUTPUT_BYTES
    );
}

fn agent_entry(status: BackgroundTaskStatus) -> AgentJobEntry {
    let mut state = JobState {
        info: entry(status).info(),
        output: VecDeque::new(),
        output_bytes: 0,
        next_seq: 0,
    };
    state.info.kind = BackgroundTaskKind::Agent;
    AgentJobEntry {
        owner: Mutex::new(owner()),
        abort: AbortSignal::new(),
        state: Mutex::new(state),
        notify: Notify::new(),
        metadata_path: PathBuf::new(),
    }
}

#[test]
fn both_job_kinds_preserve_empty_output_cursor_and_incremental_multibyte_tail() {
    let shell = entry(BackgroundTaskStatus::Running);
    let agent = agent_entry(BackgroundTaskStatus::Running);
    assert!(
        shell
            .append_output("stdout", String::new())
            .unwrap()
            .is_none()
    );
    assert!(
        agent
            .append_output("agent", String::new())
            .unwrap()
            .is_none()
    );
    for text in ["旧".repeat(MAX_OUTPUT_BYTES / 3), "新尾部".into()] {
        shell.append_output("stdout", text.clone()).unwrap();
        agent.append_output("agent", text).unwrap();
    }
    for snapshot in [shell.snapshot(1).unwrap(), agent.snapshot(1).unwrap()] {
        assert_eq!(snapshot.next_seq, 2);
        assert_eq!(snapshot.task.output_cursor, 2);
        assert!(snapshot.task.output_truncated);
        assert_eq!(snapshot.output.len(), 1);
        assert_eq!(snapshot.output[0].text, "新尾部");
    }
    assert!(shell.snapshot(2).unwrap().output.is_empty());
    assert!(agent.snapshot(2).unwrap().output.is_empty());
    let result = AgentRunResult {
        agent_name: "child".into(),
        session_id: "child-session".into(),
        status: AgentRunStatus::Completed,
        response: "done".into(),
    };
    shell.finish(BackgroundTaskStatus::Completed, Some(0), None);
    agent.finish(BackgroundTaskStatus::Completed, Some(&result), None);
    let shell = shell.snapshot(2).unwrap();
    let agent = agent.snapshot(2).unwrap();
    assert_eq!(shell.task.exit_code, Some(0));
    assert_eq!(agent.task.result.as_deref(), Some("done"));
    assert_eq!(
        agent.task.child_session_id.as_deref(),
        Some("child-session")
    );
    assert!(shell.task.completed_at.is_some() && agent.task.completed_at.is_some());
}

#[tokio::test]
async fn common_wait_returns_on_completion_for_both_kinds_and_bounds_running_wait() {
    let shell = entry(BackgroundTaskStatus::Running);
    let agent = agent_entry(BackgroundTaskStatus::Running);
    let waiter = async {
        wait_for_state(&shell.state, &shell.notify, 5_000).await;
        wait_for_state(&agent.state, &agent.notify, 5_000).await;
    };
    let completion = async {
        tokio::task::yield_now().await;
        shell.finish(BackgroundTaskStatus::Completed, Some(0), None);
        agent.finish(BackgroundTaskStatus::Terminated, None, None);
    };
    tokio::time::timeout(Duration::from_millis(500), async {
        tokio::join!(waiter, completion);
    })
    .await
    .unwrap();
    let still_running = entry(BackgroundTaskStatus::Running);
    tokio::time::timeout(
        Duration::from_millis(500),
        wait_for_state(&still_running.state, &still_running.notify, 5),
    )
    .await
    .unwrap();
}

#[test]
fn owner_check_is_strict() {
    let entry = entry(BackgroundTaskStatus::Completed);
    assert!(entry.belongs_to(&owner()));
    assert!(!entry.belongs_to(&BackgroundTaskOwner::new("agent", "other", 1)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_query_manage_and_restart_are_session_scoped() {
    let _home_guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pipi-background-{}-{}",
        std::process::id(),
        pipi_agent::session::new_id()
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    let permissions = pipi_tools::permissions::PermissionsConfig {
        tools: pipi_tools::permissions::BACKGROUND_TASK_TOOLS
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        sandbox: pipi_tools::permissions::SandboxMode::DangerFullAccess,
        ..Default::default()
    };
    pipi_core::agents::create_agent(
        "background-test",
        "background test",
        Some(workspace.to_str().unwrap()),
        Some(permissions),
        None,
        None,
    )
    .unwrap();

    let manager = BackgroundTaskManager::new(
        tokio::runtime::Handle::current(),
        Arc::new(crate::runtime::RuntimeChildRunner),
    );
    let task_owner = BackgroundTaskOwner::new("background-test", "session-a", 1);
    let info = manager
        .submit(BackgroundTaskSpec {
            owner: task_owner.clone(),
            command: "printf first; sleep 0.1; printf second".into(),
            cwd: workspace.clone(),
            env: std::collections::BTreeMap::new(),
        })
        .await
        .unwrap();
    let mut snapshot = None;
    for _ in 0..20 {
        let current = manager
            .query(
                &task_owner,
                BackgroundTaskQuery {
                    job_id: Some(info.job_id.clone()),
                    wait_ms: 100,
                    include_completed: true,
                    limit: 20,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .pop()
            .unwrap();
        if !current.task.status.is_active() {
            snapshot = Some(current);
            break;
        }
    }
    let snapshot = snapshot.expect("后台任务应在测试窗口内完成");
    assert_eq!(snapshot.task.status, BackgroundTaskStatus::Completed);
    assert!(
        snapshot
            .output
            .iter()
            .any(|chunk| chunk.text.contains("first"))
    );
    let cursor = snapshot.next_seq;
    let incremental = manager
        .query(
            &BackgroundTaskOwner::new("background-test", "session-a", 99),
            BackgroundTaskQuery {
                job_id: Some(info.job_id.clone()),
                after_seq: cursor,
                include_completed: true,
                limit: 20,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(incremental[0].output.is_empty());
    let reopened_manager = BackgroundTaskManager::new(
        tokio::runtime::Handle::current(),
        Arc::new(crate::runtime::RuntimeChildRunner),
    );
    let reopened = reopened_manager
        .query(
            &task_owner,
            BackgroundTaskQuery {
                job_id: Some(info.job_id.clone()),
                include_completed: true,
                limit: 20,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        reopened[0]
            .output
            .iter()
            .any(|chunk| chunk.text.contains("second"))
    );

    let interactive = manager
        .submit(BackgroundTaskSpec {
            owner: task_owner.clone(),
            command: "read line; printf 'got:%s' \"$line\"".into(),
            cwd: workspace.clone(),
            env: std::collections::BTreeMap::new(),
        })
        .await
        .unwrap();
    manager
        .manage(
            &task_owner,
            &interactive.job_id,
            BackgroundTaskCommand::WriteStdin("hello\n".into()),
        )
        .await
        .unwrap();
    let mut interactive_done = false;
    for _ in 0..20 {
        let current = manager
            .query(
                &task_owner,
                BackgroundTaskQuery {
                    job_id: Some(interactive.job_id.clone()),
                    wait_ms: 100,
                    include_completed: true,
                    limit: 20,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        if !current[0].task.status.is_active() {
            assert!(
                current[0]
                    .output
                    .iter()
                    .any(|chunk| chunk.text.contains("got:hello"))
            );
            interactive_done = true;
            break;
        }
    }
    assert!(interactive_done);

    let long_running = manager
        .submit(BackgroundTaskSpec {
            owner: task_owner.clone(),
            command: "sleep 30".into(),
            cwd: workspace.clone(),
            env: std::collections::BTreeMap::new(),
        })
        .await
        .unwrap();
    let restarted_manager = BackgroundTaskManager::new(
        tokio::runtime::Handle::current(),
        Arc::new(crate::runtime::RuntimeChildRunner),
    );
    let orphan = restarted_manager
        .query(
            &task_owner,
            BackgroundTaskQuery {
                job_id: Some(long_running.job_id.clone()),
                include_completed: true,
                limit: 20,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(orphan[0].task.status, BackgroundTaskStatus::Orphaned);
    assert!(restarted_manager.active_count(&task_owner) == 0);
    manager
        .manage(
            &task_owner,
            &long_running.job_id,
            BackgroundTaskCommand::Terminate,
        )
        .await
        .unwrap();

    // child Agent 也必须走同一套 submit/query/persist 语义；无模型配置让它
    // 在不依赖网络的情况下快速进入失败终态，但仍创建可读取的 child session。
    pipi_core::agents::create_agent(
        "agent-child",
        "background child",
        Some(workspace.to_str().unwrap()),
        Some(pipi_tools::permissions::PermissionsConfig {
            tools: vec!["read".into()],
            sandbox: pipi_tools::permissions::SandboxMode::WorkspaceWrite,
            ..Default::default()
        }),
        None,
        None,
    )
    .unwrap();
    let child_session_events = Arc::new(Mutex::new(Vec::<(String, String, usize)>::new()));
    let session_sink = {
        let child_session_events = child_session_events.clone();
        Arc::new(
            move |agent_name: String, session_id: String, run_id: usize| {
                child_session_events
                    .lock()
                    .unwrap()
                    .push((agent_name, session_id, run_id));
            },
        )
    };
    let child = manager
        .submit_agent(BackgroundAgentTaskSpec {
            owner: task_owner.clone(),
            target_agent: "agent-child".into(),
            prompt: "不会调用 provider".into(),
            session_sink: Some(session_sink),
        })
        .await
        .unwrap();
    assert_eq!(child.kind, BackgroundTaskKind::Agent);
    assert_eq!(child.target_agent.as_deref(), Some("agent-child"));
    let mut child_done = None;
    for _ in 0..20 {
        let current = manager
            .query(
                &task_owner,
                BackgroundTaskQuery {
                    job_id: Some(child.job_id.clone()),
                    wait_ms: 100,
                    include_completed: true,
                    limit: 20,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .pop()
            .unwrap();
        if !current.task.status.is_active() {
            child_done = Some(current);
            break;
        }
    }
    let child_done = child_done.expect("后台 Agent 任务应在测试窗口内结束");
    assert_eq!(child_done.task.kind, BackgroundTaskKind::Agent);
    assert_eq!(child_done.task.status, BackgroundTaskStatus::Failed);
    assert!(child_done.task.child_session_id.is_some());
    assert!(
        child_done
            .task
            .result
            .as_deref()
            .is_some_and(|result| result.contains("还未配置默认模型"))
    );
    let child_session_id = child_done.task.child_session_id.as_deref();
    assert!(child_session_events.lock().unwrap().iter().any(
        |(agent_name, session_id, _run_id)| {
            agent_name == "agent-child" && Some(session_id.as_str()) == child_session_id
        }
    ));
}
