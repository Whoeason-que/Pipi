//! git 载体的黑盒验收（`docs/git-session-store.md` §9 的 1–5、7、8）。
//!
//! 只用公开入口：契约层类型 + `pipi_session` 的公开 API + `git` 命令本身
//! （「标准 git 工具能读到什么」是验收的一部分）。载体一致性由
//! `tests/backend_conformance.rs` 盯，这里盯 git 特有的性质。

use pipi_protocol::session::{CompactionRecord, EntryKind, SessionEntry, SessionRepo};
use pipi_protocol::{Message, new_id, now_millis};
use pipi_session::{GitExec, GitSessionRepo, entry_path, load_session};

fn branch(id: &str) -> String {
    format!("refs/heads/session/{id}")
}

/// 追加一条挂在当前 tip 上的条目（旧 JSONL 的行内容就是它）。
fn push_entry(entries: &mut Vec<SessionEntry>, kind: EntryKind, timestamp: u64) {
    let parent_id = entries.last().map(|entry| entry.id.clone());
    entries.push(SessionEntry {
        id: new_id(),
        parent_id,
        seq: entries.len() as u64,
        timestamp,
        kind,
    });
}

/// 直接写一条落盘的 JSONL 会话（旧格式 = 导入读取的那一行 JSON），
/// 返回 (id, 文件路径, 条目)。
fn make_jsonl_session(
    dir: &std::path::Path,
) -> (
    String,
    std::path::PathBuf,
    Vec<pipi_protocol::session::SessionEntry>,
) {
    let id = format!("{}-{}", now_millis(), new_id());
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let file = sessions.join(format!("{id}.jsonl"));

    let mut entries: Vec<SessionEntry> = Vec::new();
    let user = Message::user_text("第一轮：读一下 README");
    let timestamp = user.timestamp();
    push_entry(
        &mut entries,
        EntryKind::Message { message: user },
        timestamp,
    );
    let assistant = Message::assistant_text("好的", "mock");
    let timestamp = assistant.timestamp();
    push_entry(
        &mut entries,
        EntryKind::Message { message: assistant },
        timestamp,
    );
    push_entry(
        &mut entries,
        EntryKind::Custom {
            custom_type: "mark".into(),
        },
        now_millis(),
    );

    let text: String = entries
        .iter()
        .map(|entry| format!("{}\n", serde_json::to_string(entry).unwrap()))
        .collect();
    std::fs::write(&file, text).unwrap();

    let entries = load_session(&file).unwrap();
    (id, file, entries)
}

/// §9.1 等价性：导入后 `git show` 取出的 blob 与 JSONL 文件的那一行逐字节一致。
#[test]
fn imported_entries_are_byte_identical_to_the_jsonl_lines() {
    let dir = pipi_test_support::temp_dir("pipi-git-equivalence");
    let (id, file, entries) = make_jsonl_session(&dir);
    let lines = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), entries.len());

    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    assert!(
        repo.import(&id, &entries, false).unwrap(),
        "首次导入应当写入"
    );

    let exec = GitExec::new(dir.join("sessions.git"));
    for (entry, line) in entries.iter().zip(&lines) {
        let spec = format!("{}:{}", branch(&id), entry_path(entry));
        let blob = exec.run(&["show", &spec]).unwrap();
        let mut expected = line.clone();
        expected.push('\n');
        assert_eq!(
            String::from_utf8_lossy(&blob),
            expected,
            "blob 必须就是今天落盘的那一行"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// §9.2 压缩＝分支：新分支与原线共享 tip 提交（零复制），且轮级浏览可读。
#[test]
fn compaction_style_fork_shares_the_tip_commit_and_stays_browsable() {
    let dir = pipi_test_support::temp_dir("pipi-git-compaction");
    let (id, _file, entries) = make_jsonl_session(&dir);
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    repo.import(&id, &entries, false).unwrap();
    let exec = GitExec::new(dir.join("sessions.git"));

    // 分叉（= 压缩换会话的同一机制）：新分支指向原 tip，零复制
    let forked = repo.fork(&id, None).unwrap();
    let forked_id = forked.session_id().to_string();
    drop(forked);
    let source_tip = exec.resolve_ref(&branch(&id)).unwrap().unwrap();
    let forked_tip = exec.resolve_ref(&branch(&forked_id)).unwrap().unwrap();
    assert_eq!(source_tip, forked_tip, "新分支必须指向同一个提交");
    let count = |reference: &str| {
        String::from_utf8_lossy(&exec.run(&["rev-list", "--count", reference]).unwrap())
            .trim()
            .to_string()
    };
    assert_eq!(count(&branch(&id)), count(&branch(&forked_id)));

    // 标准 git 工具能读：一条 commit 一轮、subject 是 user 摘要、带 Turn trailer
    let subjects =
        String::from_utf8_lossy(&exec.run(&["log", "--format=%s", &branch(&id)]).unwrap())
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
    assert_eq!(subjects.len(), 1, "一轮 = 一个提交");
    assert_eq!(subjects[0], "user: 第一轮：读一下 README");
    let body = String::from_utf8_lossy(&exec.run(&["log", "--format=%B", &branch(&id)]).unwrap())
        .to_string();
    assert!(body.contains("Turn: 1"), "{body}");
    let grep = String::from_utf8_lossy(
        &exec
            .run(&["log", "--oneline", "--grep=Turn: 1", &branch(&id)])
            .unwrap(),
    )
    .lines()
    .count();
    assert_eq!(grep, 1, "轮级浏览：--grep 命中该轮");

    // 两条线各自可读，内容一致（保留段对象被两条线共享）
    assert_eq!(repo.entries(&id).unwrap().len(), entries.len());
    assert_eq!(repo.entries(&forked_id).unwrap().len(), entries.len());

    // 整线分叉没有新提交，来源靠元数据 ref 记录
    let marker = format!("refs/pipi/forks/{forked_id}/{id}");
    assert!(
        exec.ref_exists(&marker).unwrap(),
        "分叉来源 ref 必须指向源会话"
    );
    let summaries = repo.list();
    let forked_summary = summaries
        .iter()
        .find(|summary| summary.id == forked_id)
        .unwrap();
    assert_eq!(forked_summary.fork_parent_id.as_deref(), Some(id.as_str()));
    let source_summary = summaries.iter().find(|summary| summary.id == id).unwrap();
    assert_eq!(source_summary.fork_parent_id, None);
    std::fs::remove_dir_all(&dir).ok();
}

/// 删除分叉会话时先清来源 ref：否则 marker 会保活对象，`gc --prune=now` 失效。
#[test]
fn deleting_a_forked_session_removes_its_lineage_ref() {
    let dir = pipi_test_support::temp_dir("pipi-git-fork-delete");
    let (id, _file, entries) = make_jsonl_session(&dir);
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    repo.import(&id, &entries, false).unwrap();
    let forked = repo.fork(&id, None).unwrap();
    let forked_id = forked.session_id().to_string();
    drop(forked);
    let exec = GitExec::new(dir.join("sessions.git"));
    let marker = format!("refs/pipi/forks/{forked_id}/{id}");
    assert!(exec.ref_exists(&marker).unwrap());

    repo.delete(&forked_id).unwrap();
    assert!(
        !exec.ref_exists(&marker).unwrap(),
        "删除会话必须同时清掉来源 ref"
    );
    assert!(
        repo.list().iter().all(|summary| summary.id != forked_id),
        "删除后不再出现在列表里"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// §9.4 删除：删 ref 之后独有对象不可读（`gc --prune=now` 已跑）。
#[test]
fn delete_prunes_the_objects_only_that_session_referenced() {
    let dir = pipi_test_support::temp_dir("pipi-git-delete");
    let (id, _file, entries) = make_jsonl_session(&dir);
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    repo.import(&id, &entries, false).unwrap();
    let exec = GitExec::new(dir.join("sessions.git"));

    // 记下该会话独有 blob 的 sha
    let ls = exec.run(&["ls-tree", "-r", &branch(&id)]).unwrap();
    let blob = String::from_utf8_lossy(&ls)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(2).map(str::to_string))
        .expect("至少一个 blob");
    assert!(exec.run(&["cat-file", "-e", &blob]).is_ok(), "删除前可读");

    repo.delete(&id).unwrap();
    assert!(!repo.exists(&id).unwrap());
    assert!(
        exec.run(&["cat-file", "-e", &blob]).is_err(),
        "删除 + prune 之后独有对象不可读"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// §9.5 分发：bundle → fetch 之后接收方能按同一套读法读到内容。
#[test]
fn bundle_roundtrip_reaches_a_receiver() {
    let dir = pipi_test_support::temp_dir("pipi-git-bundle");
    let (id, _file, entries) = make_jsonl_session(&dir);
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    repo.import(&id, &entries, false).unwrap();
    let exec = GitExec::new(dir.join("sessions.git"));

    let bundle = dir.join("session.bundle");
    exec.run(&["bundle", "create", bundle.to_str().unwrap(), &branch(&id)])
        .unwrap();

    // 接收方：一个独立的 bare 仓库，fetch 之后用同一套读取路径读回
    let receiver_dir = dir.join("receiver.git");
    let status = std::process::Command::new("git")
        .args([
            "init",
            "--bare",
            "--quiet",
            "--initial-branch=main",
            receiver_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .args([
            "--git-dir",
            receiver_dir.to_str().unwrap(),
            "fetch",
            "--quiet",
            bundle.to_str().unwrap(),
            &format!("{}:{}", branch(&id), branch(&id)),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let receiver = GitSessionRepo::new(receiver_dir);
    let received = receiver.entries(&id).unwrap();
    assert_eq!(received.len(), entries.len());
    assert_eq!(
        serde_json::to_string(&received).unwrap(),
        serde_json::to_string(&entries).unwrap(),
        "接收方读到的条目与源一致"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// §9.6 缺 git / 仓库不可信：fail-closed，不静默降级成别的路径。
#[test]
fn unusable_repo_fails_closed_instead_of_falling_back() {
    let outside = pipi_test_support::temp_dir("pipi-git-outside");
    let parent = pipi_test_support::temp_dir("pipi-git-agent");
    let link = parent.join("sessions.git");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let repo = GitSessionRepo::new(link);
    let error = match repo.create() {
        Err(error) => error,
        Ok(_) => panic!("仓库目录是符号链接时必须拒绝建会话"),
    };
    assert!(
        error.contains("cannot be a symbolic link") || error.contains("不能是符号链接"),
        "{error}"
    );
    std::fs::remove_dir_all(&parent).ok();
    std::fs::remove_dir_all(&outside).ok();
}

/// §9.7 并发：同一个 Agent 的两条会话并行提交，内容互不串。
#[test]
fn parallel_sessions_commit_without_crossing() {
    let dir = pipi_test_support::temp_dir("pipi-git-parallel");
    let repo = std::sync::Arc::new(GitSessionRepo::new(dir.join("sessions.git")));

    let mut handles = Vec::new();
    for lane in 0..2 {
        let repo = repo.clone();
        handles.push(std::thread::spawn(move || {
            let mut sink = repo.create().unwrap();
            let id = sink.session_id().to_string();
            for turn in 0..3 {
                sink.append_message(&Message::user_text(format!("lane {lane} turn {turn}")))
                    .unwrap();
                sink.commit_point().unwrap();
            }
            id
        }));
    }
    let ids = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_ne!(ids[0], ids[1]);

    for (lane, id) in ids.iter().enumerate() {
        let entries = repo.entries(id).unwrap();
        assert_eq!(entries.len(), 3, "每条线只该有自己的三条条目");
        for entry in &entries {
            let pipi_protocol::session::EntryKind::Message {
                message: Message::User { content, .. },
            } = &entry.kind
            else {
                panic!("应当是用户消息");
            };
            assert!(
                content.starts_with(&format!("lane {lane} ")),
                "内容串线了：{content}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// 崩溃后恢复未提交的轮次，显式提交前不会产生额外 commit。
#[test]
fn a_crash_recovers_the_uncommitted_turn() {
    let dir = pipi_test_support::temp_dir("pipi-git-crash");
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    let mut sink = repo.create().unwrap();
    let id = sink.session_id().to_string();
    sink.append_message(&Message::user_text("已经提交的一轮"))
        .unwrap();
    sink.commit_point().unwrap();
    sink.append_message(&Message::user_text("还没提交就崩了"))
        .unwrap();
    drop(sink); // 进程被杀：提交点不会执行

    let entries = repo.entries(&id).unwrap();
    assert_eq!(entries.len(), 2);
    let mut reopened = repo.open(&id).unwrap();
    assert!(reopened.has_recovery());
    assert_eq!(reopened.tip_id(), Some(entries[1].id.as_str()));
    assert_eq!(repo.history(&id).unwrap().len(), 1);
    reopened.commit_point().unwrap();
    assert!(!reopened.has_recovery());
    assert_eq!(repo.history(&id).unwrap().len(), 2);
    assert_eq!(repo.entries(&id).unwrap().len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

/// 真实会话语料（手动跑，抄 `pipi-context` 的 `compaction_corpus` 口径）：
///
/// ```text
/// PIPI_DOGFOOD_SESSION=~/.pipi/agents/Q/sessions/<id>.jsonl \
///   cargo test -p pipi-session --test git_backend -- --ignored --nocapture
/// ```
///
/// 只读真实文件、只写临时仓库，不碰用户数据。
#[test]
#[ignore = "需要 PIPI_DOGFOOD_SESSION 指向一条真实会话 JSONL"]
fn dogfood_a_real_session_roundtrips_through_git() {
    let Some(path) = std::env::var_os("PIPI_DOGFOOD_SESSION") else {
        panic!("请设置 PIPI_DOGFOOD_SESSION");
    };
    let source = std::path::PathBuf::from(path);
    let entries = load_session(&source).expect("读取真实会话");
    let id = source
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("会话 id")
        .to_string();
    let bytes = std::fs::metadata(&source).unwrap().len();

    let dir = pipi_test_support::temp_dir("pipi-dogfood");
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    let started = std::time::Instant::now();
    assert!(
        repo.import(&id, &entries, false).unwrap(),
        "首次导入应当写入"
    );
    let imported_ms = started.elapsed().as_millis();

    let read_back = repo.entries(&id).unwrap();
    assert_eq!(read_back.len(), entries.len(), "条目数一致");
    assert_eq!(
        serde_json::to_string(&read_back).unwrap(),
        serde_json::to_string(&entries).unwrap(),
        "读回与原文逐条一致"
    );

    let exec = GitExec::new(dir.join("sessions.git"));
    let log = String::from_utf8_lossy(&exec.run(&["log", "--oneline", &branch(&id)]).unwrap())
        .lines()
        .count();
    let summary = repo
        .list()
        .into_iter()
        .find(|summary| summary.id == id)
        .expect("列表里有它");
    let on_disk = dir_size(&dir.join("sessions.git"));
    println!(
        "真实会话 {}：{} 条 / {:.2} MB；导入 {} ms；git 里 {} 个 commit；读回 {} 条；\
         标题「{}」；消息 {} 条；模型 {:?}；仓库占盘 {:.2} MB",
        id,
        entries.len(),
        bytes as f64 / 1_048_576.0,
        imported_ms,
        log,
        read_back.len(),
        summary.title,
        summary.message_count,
        summary.model,
        on_disk as f64 / 1_048_576.0,
    );
    // 标准 git 工具的观感（验收的一部分）：一行一轮、可 grep 轮号
    let head = String::from_utf8_lossy(
        &exec
            .run(&["log", "--format=%s", "--max-count=5", &branch(&id)])
            .unwrap(),
    )
    .to_string();
    let turn12 = String::from_utf8_lossy(
        &exec
            .run(&["log", "--oneline", "--grep=Turn: 12", &branch(&id)])
            .unwrap(),
    )
    .lines()
    .count();
    println!("最近 5 轮（git log --format=%s）：\n{head}第 12 轮命中 {turn12} 个 commit");
    assert!(log >= 1);
    std::fs::remove_dir_all(&dir).ok();
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

#[test]
fn uncommitted_compaction_entry_can_be_rolled_back() {
    let dir = pipi_test_support::temp_dir("pipi-git-rollback");
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    let mut log = repo.create().expect("create session");
    let session_id = log.session_id().to_string();
    let kept = log
        .append_message(&Message::user_text("keep me"))
        .expect("append message");
    log.commit_point().expect("commit message");
    let source_tip = log.tip_id().expect("tip").to_string();

    log.append_compaction(&CompactionRecord {
        summary: "discarded summary",
        strategy: "test",
        keep_from_entry: &kept,
        source_tip: &source_tip,
        usage: None,
    })
    .expect("append compaction");
    assert!(log.rollback_last_entry().expect("rollback"));
    log.commit_point().expect("commit after rollback");
    drop(log);

    let entries = repo.entries(&session_id).expect("read entries");
    assert_eq!(
        entries.len(),
        1,
        "rolled back compaction must not reach git"
    );
    assert!(matches!(entries[0].kind, EntryKind::Message { .. }));
    std::fs::remove_dir_all(dir).ok();
}

/// 一次性子运行标记：首条条目落成 `subagent_run`，列表摘要带父会话 id；
/// 普通会话没有这个字段。
#[test]
fn subagent_run_marker_is_listed_with_its_parent_session() {
    let dir = pipi_test_support::temp_dir("pipi-git-subagent-run");
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    let parent_session = format!("{}-{}", now_millis(), new_id());

    let mut log = repo.create().expect("create child session");
    let child_id = log.session_id().to_string();
    log.append_subagent_run("caller", &parent_session)
        .expect("append marker");
    log.append_message(&Message::user_text("one-off task"))
        .expect("append message");
    log.commit_point().expect("commit child session");
    drop(log);

    let mut top = repo.create().expect("create top-level session");
    let top_id = top.session_id().to_string();
    top.append_message(&Message::user_text("top-level task"))
        .expect("append message");
    top.commit_point().expect("commit top-level session");
    drop(top);

    let summaries = repo.list();
    let child = summaries
        .iter()
        .find(|summary| summary.id == child_id)
        .expect("child summary");
    assert_eq!(
        child.parent_session_id.as_deref(),
        Some(parent_session.as_str())
    );
    let top = summaries
        .iter()
        .find(|summary| summary.id == top_id)
        .expect("top-level summary");
    assert_eq!(top.parent_session_id, None);

    let entries = repo.entries(&child_id).expect("read child entries");
    assert!(matches!(
        entries.first().map(|entry| &entry.kind),
        Some(EntryKind::SubagentRun { parent_agent, .. }) if parent_agent == "caller"
    ));
    std::fs::remove_dir_all(dir).ok();
}

/// 右栏 git tree 的只读历史：一轮一个提交，带轮号、新增条目与 tip 标记。
#[test]
fn history_lists_turn_commits_and_entry_kinds() {
    let dir = pipi_test_support::temp_dir("pipi-git-history");
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    let mut sink = repo.create().expect("创建会话");
    let id = sink.session_id().to_string();

    sink.append_message(&Message::user_text("第一轮：读 README"))
        .unwrap();
    sink.append_message(&Message::assistant_text("好的", "mock"))
        .unwrap();
    sink.commit_point().expect("提交第一轮");

    sink.append_message(&Message::user_text("第二轮：压缩"))
        .unwrap();
    sink.append_compaction(&CompactionRecord {
        summary: "前一轮摘要",
        strategy: "llm-summarize",
        keep_from_entry: "",
        source_tip: "",
        usage: None,
    })
    .unwrap();
    sink.commit_point().expect("提交第二轮");
    drop(sink);

    let history = repo.history(&id).expect("读取历史");
    assert_eq!(history.len(), 2, "两轮两个提交");
    // git log 默认新→旧
    assert_eq!(history[0].summary, "user: 第二轮：压缩");
    assert_eq!(history[0].turn, Some(2));
    assert!(history[0].is_tip);
    assert!(!history[1].is_tip);
    assert_eq!(
        history[0].parent_id.as_deref(),
        Some(history[1].id.as_str())
    );
    assert!(history[0].timestamp > 0);
    let kinds: Vec<&str> = history[0]
        .entries
        .iter()
        .map(|entry| entry.kind.as_str())
        .collect();
    assert_eq!(kinds, vec!["message", "compaction"]);
    assert!(
        history[0].entries.iter().all(|entry| entry.id.len() == 8),
        "树里展示 8 位短 id"
    );

    // 不存在的会话返回空历史，不报错（右栏可以安全地显示空树）。
    assert!(repo.history("0-missing").unwrap().is_empty());
    std::fs::remove_dir_all(dir).ok();
}
