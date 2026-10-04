use super::*;

#[test]
fn timeout_validation() {
    assert!(validate_timeout(None).unwrap().is_none());
    assert!(validate_timeout(Some(10.0)).unwrap() == Some(10));
    assert!(validate_timeout(Some(0.0)).is_err());
    assert!(validate_timeout(Some(-1.0)).is_err());
    assert!(validate_timeout(Some(f64::NAN)).is_err());
    assert!(validate_timeout(Some(MAX_TIMEOUT_SECONDS + 1.0)).is_err());
}

/// av 契约接线：子进程环境完全来自 resolved_env（env_clear + envs）——
/// 声明外的变量（含 PATH）不可见。
#[tokio::test]
async fn bash_spawns_with_resolved_env_only() {
    let workspace =
        std::env::temp_dir().join(format!("pipi-bash-env-{}", crate::session::new_id()));
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let resolved =
        std::collections::BTreeMap::from([("AV_MARKER".to_string(), "from-av".to_string())]);
    let ctx = ToolContext {
        workspace,
        memory_dir: None,
        read_roots: Vec::new(),
        permissions: std::sync::Arc::new(crate::permissions::PermissionsConfig::default()),
        sandbox: crate::permissions::SandboxMode::DangerFullAccess,
        resolved_env: std::sync::Arc::new(resolved),
        abort: crate::types::AbortSignal::new(),
        approver: None,
    };
    let output = BashTool
        .execute(
            &ctx,
            // bash 在 PATH 缺失时会自行合成默认 PATH，故用自定义变量名验证隔离
            &serde_json::json!({ "command": "echo \"$AV_MARKER:${PIPI_AV_TEST_LEAK:-absent}\"" }),
            &|_| {},
        )
        .await
        .unwrap();
    let text = match &output.content[0] {
        ToolResultContent::Text { text } => text.clone(),
        _ => panic!("expected text output"),
    };
    assert_eq!(text.trim(), "from-av:absent");
}

/// 建一个最小 ToolContext（bash 用）。
fn test_context(workspace: std::path::PathBuf, abort: crate::types::AbortSignal) -> ToolContext {
    ToolContext {
        workspace,
        memory_dir: None,
        read_roots: Vec::new(),
        permissions: std::sync::Arc::new(crate::permissions::PermissionsConfig::default()),
        sandbox: crate::permissions::SandboxMode::DangerFullAccess,
        resolved_env: std::sync::Arc::new(std::collections::BTreeMap::new()),
        abort,
        approver: None,
    }
}

fn test_workspace(tag: &str) -> std::path::PathBuf {
    let workspace =
        std::env::temp_dir().join(format!("pipi-bash-{tag}-{}", crate::session::new_id()));
    std::fs::create_dir_all(&workspace).unwrap();
    workspace
}

/// 进程是否已经终止（消失，或在等 init 回收的僵尸态）。
#[cfg(unix)]
fn process_terminated(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // `pid (comm) state ...`：comm 里可能有 ')'，取最后一个 ')' 之后的部分
            Ok(stat) => stat
                .rsplit(')')
                .next()
                .map(|rest| rest.trim_start().starts_with('Z'))
                .unwrap_or(false),
            Err(_) => true,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        kill(Pid::from_raw(pid), None).is_err()
    }
}

#[cfg(unix)]
async fn assert_process_terminated(pid: i32) {
    for _ in 0..150 {
        if process_terminated(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("孙进程 {pid} 仍在运行：进程组没有被完整终止");
}

fn read_pid_file(workspace: &std::path::Path) -> i32 {
    let text = std::fs::read_to_string(workspace.join("bg.pid")).expect("后台进程 pid 文件");
    text.trim().parse().expect("pid")
}

/// 超时不能只杀直接子进程：后台孙进程继续握着 stdout/stderr 时，
/// 读取任务等不到 EOF（工具卡死），孙进程还留在系统里。
#[cfg(unix)]
#[tokio::test]
async fn timeout_terminates_the_whole_process_tree() {
    let workspace = test_workspace("timeout-tree");
    let ctx = test_context(workspace.clone(), crate::types::AbortSignal::new());
    let result = BashTool
        .execute(
            &ctx,
            &serde_json::json!({
                "command": "sleep 30 & echo $! > bg.pid; wait",
                "timeout": 1
            }),
            &|_| {},
        )
        .await;
    let error = result.expect_err("超时应报错");
    assert!(error.contains("timed out"), "错误信息应说明超时: {error}");
    assert_process_terminated(read_pid_file(&workspace)).await;
    std::fs::remove_dir_all(&workspace).ok();
}

/// 中止（停止按钮）同样终止整棵树，并且立刻返回 ——
/// 旧实现要等下一次 100ms 轮询，且会被孙进程拖住。
#[cfg(unix)]
#[tokio::test]
async fn abort_terminates_the_whole_process_tree() {
    let workspace = test_workspace("abort-tree");
    let abort = crate::types::AbortSignal::new();
    let task = tokio::spawn({
        let workspace = workspace.clone();
        let abort = abort.clone();
        async move {
            let ctx = test_context(workspace, abort);
            BashTool
                .execute(
                    &ctx,
                    &serde_json::json!({ "command": "sleep 30 & echo $! > bg.pid; wait" }),
                    &|_| {},
                )
                .await
        }
    });
    // 等命令真正跑起来（pid 文件是 shell 写的）
    tokio::time::sleep(Duration::from_millis(400)).await;
    let started = Instant::now();
    abort.abort();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("中止后应立即返回，不能被孙进程拖住")
        .expect("任务不应 panic");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "中止收尾不应耗时过久"
    );
    let error = result.expect_err("中止应报错");
    assert!(error.contains("aborted"), "错误信息应说明中止: {error}");
    assert_process_terminated(read_pid_file(&workspace)).await;
    std::fs::remove_dir_all(&workspace).ok();
}
