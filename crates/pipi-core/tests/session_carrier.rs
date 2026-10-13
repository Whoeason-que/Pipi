//! 载体装配的端到端：新建走 git 分支，旧 JSONL 会话按 id 兼容（可继续对话），
//! 显式导入把它们迁到 git（逐条校验、幂等、原文件改名保留），归档/删除对两种
//! 载体都成立。
//!
//! 改写 HOME 前持 `pipi_test_support::HOME_LOCK`（全仓唯一的一把锁）。

use std::sync::Arc;

use pipi_protocol::Message;
use pipi_protocol::permissions::{BashPermissions, PermissionsConfig, SandboxMode};
use pipi_protocol::session::SessionRepo;
use pipi_session::{AgentSessionRepo, SessionWriter};
use pipi_venv::agents;

/// 造一个 Agent 与它的目录（返回 Agent 数据目录）。
fn fixture_agent(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!(
        "pipi-carrier-{}-{}",
        std::process::id(),
        pipi_protocol::new_id()
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    agents::create_agent(
        name,
        "载体装配用例",
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
    let definition = agents::load_agent(name).unwrap();
    definition
        .sessions_dir()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn repo(agent_dir: &std::path::Path) -> AgentSessionRepo {
    AgentSessionRepo::new(agent_dir)
}

/// 旧 JSONL 会话（历史遗留）：用 JSONL 载体直接播种。
fn seed_legacy(agent_dir: &std::path::Path, first_message: &str) -> String {
    let mut writer = SessionWriter::create(&agent_dir.join("sessions")).unwrap();
    writer
        .append_message(&Message::user_text(first_message))
        .unwrap();
    writer.session_id().to_string()
}

/// 归档区的旧会话（导入后必须仍留在归档区）。
fn seed_archived_legacy(agent_dir: &std::path::Path, first_message: &str) -> String {
    let id = seed_legacy(agent_dir, first_message);
    let source = agent_dir.join("sessions").join(format!("{id}.jsonl"));
    let archive = agent_dir.join("sessions").join(".archive");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::rename(&source, archive.join(format!("{id}.jsonl"))).unwrap();
    id
}

#[test]
fn new_sessions_land_in_git_and_legacy_ones_stay_readable_until_imported() {
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let agent_dir = fixture_agent("carrier-mixed");
    let repo = repo(&agent_dir);

    // 新建走 git：分支要等提交点（会话创建那次）才出现
    let mut fresh = repo.create().unwrap();
    let fresh_id = fresh.session_id().to_string();
    fresh
        .append_message(&Message::user_text("新会话在 git 里"))
        .unwrap();
    fresh.commit_point().unwrap();
    drop(fresh);
    assert!(
        repo.git().exists(&fresh_id).unwrap(),
        "新会话应当落在 git 分支"
    );
    assert!(
        !agent_dir
            .join("sessions")
            .join(format!("{fresh_id}.jsonl"))
            .exists(),
        "新会话不该再写 JSONL 文件"
    );

    // 旧会话照旧可读、可继续（同一个会话只有一个载体）
    let legacy_id = seed_legacy(&agent_dir, "旧会话的第一句");
    assert!(repo.exists(&legacy_id).unwrap());
    let mut resumed = repo.open(&legacy_id).unwrap();
    resumed
        .append_message(&Message::user_text("旧会话继续对话"))
        .unwrap();
    resumed.commit_point().unwrap();
    drop(resumed);
    let entries = repo.entries(&legacy_id).unwrap();
    assert_eq!(entries.len(), 2, "旧会话继续写在自己的 JSONL 里");
    assert!(
        !repo.git().exists(&legacy_id).unwrap(),
        "未导入前 git 里没有它"
    );

    // 列表合并两个载体
    let listed = repo.list();
    assert!(listed.iter().any(|summary| summary.id == fresh_id));
    assert!(listed.iter().any(|summary| summary.id == legacy_id));
    let legacy_summary = listed
        .iter()
        .find(|summary| summary.id == legacy_id)
        .unwrap();
    assert_eq!(legacy_summary.title, "旧会话的第一句");
    assert_eq!(legacy_summary.message_count, 2);

    // 归档区的旧会话：导入后必须仍留在归档区
    let archived_id = seed_archived_legacy(&agent_dir, "归档的旧会话");
    let report = repo.import_legacy_sessions();
    assert!(
        report.imported.contains(&archived_id),
        "归档会话也要导入：{:?}",
        report
    );
    assert!(
        repo.list_archived()
            .unwrap()
            .iter()
            .any(|summary| summary.id == archived_id),
        "导入后应留在归档区"
    );
    assert!(
        !repo.exists(&archived_id).unwrap(),
        "导入不该把归档会话搬回活跃区"
    );
    // 内容经「先恢复再读」验一遍（与旧载体的产品流程一致：归档区只出摘要）
    repo.restore(&archived_id).unwrap();
    assert_eq!(repo.entries(&archived_id).unwrap().len(), 1);
    repo.archive(&archived_id).unwrap();

    // 导入：迁到 git、原文件改名保留、幂等
    assert_eq!(
        report.imported,
        vec![legacy_id.clone(), archived_id.clone()]
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(repo.git().exists(&legacy_id).unwrap());
    assert_eq!(repo.entries(&legacy_id).unwrap().len(), 2);
    assert!(
        agent_dir
            .join("sessions")
            .join(format!("{legacy_id}.jsonl.imported"))
            .is_file(),
        "原文件应当改名保留"
    );
    let again = repo.import_legacy_sessions();
    assert!(again.imported.is_empty(), "重复导入幂等：{:?}", again);
    assert!(again.failed.is_empty());

    // 导入后列表仍能看到它（且只有一条，不重复）
    let listed = repo.list();
    assert_eq!(
        listed
            .iter()
            .filter(|summary| summary.id == legacy_id)
            .count(),
        1
    );
    std::fs::remove_dir_all(agent_dir.parent().unwrap()).ok();
}

#[test]
fn archive_restore_and_delete_work_on_git_backed_sessions() {
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let agent_dir = fixture_agent("carrier-lifecycle");
    let repo = repo(&agent_dir);

    let mut sink = repo.create().unwrap();
    let id = sink.session_id().to_string();
    sink.append_message(&Message::user_text("要被归档的会话"))
        .unwrap();
    sink.commit_point().unwrap();
    drop(sink);

    repo.archive(&id).unwrap();
    assert!(!repo.exists(&id).unwrap());
    assert!(
        repo.list_archived()
            .unwrap()
            .iter()
            .any(|summary| summary.id == id)
    );
    repo.restore(&id).unwrap();
    assert!(repo.exists(&id).unwrap());
    assert_eq!(repo.entries(&id).unwrap().len(), 1);

    repo.delete(&id).unwrap();
    assert!(!repo.exists(&id).unwrap());
    assert!(repo.entries(&id).unwrap_err().contains("不存在"));
    std::fs::remove_dir_all(agent_dir.parent().unwrap()).ok();
}

#[test]
fn the_host_imports_legacy_sessions_through_the_command() {
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let agent_dir = fixture_agent("carrier-command");
    let legacy_id = seed_legacy(&agent_dir, "历史会话");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let state = pipi_core::runtime::RuntimeState::new(runtime.handle().clone());
    let report = runtime
        .block_on(pipi_core::commands::import_sessions(
            &state,
            pipi_core::commands::ImportSessionsArgs {
                agent_name: "carrier-command".into(),
            },
        ))
        .expect("导入命令应当成功");
    assert_eq!(report.imported, vec![legacy_id.clone()]);

    // 导入后宿主列表里仍然只有一条，且读得到内容
    let listed = pipi_core::runtime::list_sessions("carrier-command").unwrap();
    assert_eq!(
        listed
            .iter()
            .filter(|summary| summary.id == legacy_id)
            .count(),
        1
    );
    let repo = repo(&agent_dir);
    assert_eq!(repo.entries(&legacy_id).unwrap().len(), 1);
    std::fs::remove_dir_all(agent_dir.parent().unwrap()).ok();
}

/// 编译期锚：装配点返回的仓库实现了端口（命令与运行时都只经端口用它）。
#[allow(dead_code)]
fn assert_repo_is_a_port(repo: &dyn SessionRepo) -> usize {
    repo.list().len()
}

#[allow(dead_code)]
fn assert_arc_is_a_port(repo: Arc<dyn SessionRepo>) -> usize {
    repo.list().len()
}
