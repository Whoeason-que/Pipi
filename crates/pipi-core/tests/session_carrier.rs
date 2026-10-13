//! 载体装配的端到端：git 分支是唯一持久载体 —— 新建、续写、归档、删除都在
//! 分支上；未迁移机器上的旧 `sessions/*.jsonl` 只经显式导入命令迁进 git，
//! 导入后原文件改名保留、幂等。
//!
//! 改写 HOME 前持 `pipi_test_support::HOME_LOCK`（全仓唯一的一把锁）。

use pipi_protocol::Message;
use pipi_protocol::permissions::{BashPermissions, PermissionsConfig, SandboxMode};
use pipi_protocol::session::{EntryKind, SessionEntry, SessionRepo};
use pipi_session::{GitSessionRepo, import_legacy_sessions};
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

fn repo(agent_dir: &std::path::Path) -> GitSessionRepo {
    GitSessionRepo::new(agent_dir.join("sessions.git"))
}

/// 直接写一条旧格式的 JSONL 会话（导入读的就是这一行 JSON），返回它的 id。
fn seed_legacy(agent_dir: &std::path::Path, first_message: &str) -> String {
    let id = format!(
        "{}-{}",
        pipi_protocol::now_millis(),
        pipi_protocol::new_id()
    );
    let message = Message::user_text(first_message);
    let entry = SessionEntry {
        id: pipi_protocol::new_id(),
        parent_id: None,
        seq: 0,
        timestamp: message.timestamp(),
        kind: EntryKind::Message { message },
    };
    let dir = agent_dir.join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.jsonl")),
        format!("{}\n", serde_json::to_string(&entry).unwrap()),
    )
    .unwrap();
    id
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
fn new_sessions_land_in_git_and_legacy_ones_need_an_explicit_import() {
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
    assert!(repo.exists(&fresh_id).unwrap(), "新会话应当落在 git 分支");
    assert!(
        !agent_dir
            .join("sessions")
            .join(format!("{fresh_id}.jsonl"))
            .exists(),
        "新会话不该再写 JSONL 文件"
    );

    // 旧文件不再是运行时载体：未导入前既不可读、也不进列表
    let legacy_id = seed_legacy(&agent_dir, "旧会话的第一句");
    assert!(!repo.exists(&legacy_id).unwrap());
    assert!(repo.open(&legacy_id).is_err(), "未导入的旧会话应当报不存在");
    let listed = repo.list();
    assert!(listed.iter().any(|summary| summary.id == fresh_id));
    assert!(listed.iter().all(|summary| summary.id != legacy_id));

    // 归档区的旧会话：导入后必须仍留在归档区
    let archived_id = seed_archived_legacy(&agent_dir, "归档的旧会话");
    let legacy_dir = agent_dir.join("sessions");
    let report = import_legacy_sessions(&repo, &legacy_dir);
    assert!(
        report.imported.contains(&archived_id) && report.imported.contains(&legacy_id),
        "活跃与归档的旧会话都要导入：{report:?}"
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
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
    // 内容经「先恢复再读」验一遍（与产品流程一致：归档区只出摘要）
    repo.restore(&archived_id).unwrap();
    assert_eq!(repo.entries(&archived_id).unwrap().len(), 1);
    repo.archive(&archived_id).unwrap();

    // 导入后：旧会话改由 git 提供、原文件改名保留、再跑一次幂等
    assert!(repo.exists(&legacy_id).unwrap());
    assert_eq!(repo.entries(&legacy_id).unwrap().len(), 1);
    assert!(
        agent_dir
            .join("sessions")
            .join(format!("{legacy_id}.jsonl.imported"))
            .is_file(),
        "原文件应当改名保留"
    );
    let again = import_legacy_sessions(&repo, &legacy_dir);
    assert!(again.imported.is_empty(), "重复导入幂等：{again:?}");
    assert!(again.failed.is_empty(), "{again:?}");

    // 导入后列表里只有一条，标题与内容来自分支
    let listed = repo.list();
    assert_eq!(
        listed
            .iter()
            .filter(|summary| summary.id == legacy_id)
            .count(),
        1
    );
    let legacy_summary = listed
        .iter()
        .find(|summary| summary.id == legacy_id)
        .unwrap();
    assert_eq!(legacy_summary.title, "旧会话的第一句");
    assert_eq!(legacy_summary.message_count, 1);
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
