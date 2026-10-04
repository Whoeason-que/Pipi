use super::*;
use pipi_protocol::ContentBlock;

#[test]
fn temporary_writer_keeps_ledger_state_without_a_file() {
    let mut writer = SessionWriter::temporary();
    let session_id = writer.session_id().to_string();
    assert!(session_id.starts_with("test-"));
    assert!(writer.is_temporary());
    assert!(writer.persistent_path().is_none());

    let first = writer
        .append_message(&Message::user_text("只在内存里"))
        .unwrap();
    let second = writer
        .append_message(&Message::assistant_text("收到", "mock"))
        .unwrap();

    assert_eq!(writer.tip_id(), Some(second.as_str()));
    assert_eq!(writer.message_ids(), &[first, second]);
    assert!(writer.persistent_path().is_none());
}

#[test]
fn append_and_load_roundtrip() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();

    let m1 = Message::user_text("hello");
    let m2 = Message::Assistant {
        content: vec![ContentBlock::Text { text: "hi".into() }],
        api: "test".into(),
        provider: "test".into(),
        model: "m".into(),
        usage: Default::default(),
        stop_reason: pipi_protocol::StopReason::Stop,
        error_message: None,
        timestamp: now_millis(),
        duration_ms: None,
    };
    writer.append_message(&m1).unwrap();
    writer.append_message(&m2).unwrap();
    writer.append_custom("bookmark").unwrap();

    let entries = load_session(writer.path()).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries[0].parent_id.is_none());
    assert_eq!(
        entries[1].parent_id.as_deref(),
        Some(entries[0].id.as_str())
    );
    assert_eq!(
        entries[2].parent_id.as_deref(),
        Some(entries[1].id.as_str())
    );
    assert_eq!(entries[0].seq, 0);
    assert_eq!(entries[2].seq, 2);
    match &entries[0].kind {
        EntryKind::Message { message } => assert_eq!(message.role(), "user"),
        _ => panic!("expected message entry"),
    }
    match &entries[2].kind {
        EntryKind::Custom { custom_type } => assert_eq!(custom_type, "bookmark"),
        _ => panic!("expected custom entry"),
    }
}

#[test]
fn load_skips_corrupt_lines() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("good")).unwrap();
    let path = writer.path().to_path_buf();
    // 模拟崩溃留下的半行
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, r#"{{"id":"broken""#).unwrap();
    drop(f);

    let entries = load_session(&path).unwrap();
    assert_eq!(entries.len(), 1);
}

#[test]
fn open_resumes_append_position() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("first")).unwrap();
    let path = writer.path().to_path_buf();
    drop(writer);

    // 重新打开：tip/seq 恢复，新条目接在后面
    let mut resumed = SessionWriter::open(&path).unwrap();
    assert_eq!(resumed.tip_id().is_some(), true);
    resumed
        .append_message(&Message::user_text("second"))
        .unwrap();

    let entries = load_session(&path).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].seq, 1);
    assert_eq!(
        entries[1].parent_id.as_deref(),
        Some(entries[0].id.as_str())
    );
}

#[test]
fn summaries_list_titles_and_order() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut w1 = SessionWriter::create(&dir).unwrap();
    w1.append_message(&Message::user_text("第一条会话的标题"))
        .unwrap();
    w1.append_message(&Message::user_text("second")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut w2 = SessionWriter::create(&dir).unwrap();
    w2.append_message(&Message::user_text("更新会话")).unwrap();

    let summaries = list_session_summaries(&dir);
    assert_eq!(summaries.len(), 2);
    // 按最后活跃倒序：w2 在前
    assert_eq!(summaries[0].title, "更新会话");
    assert_eq!(summaries[1].title, "第一条会话的标题");
    assert_eq!(summaries[1].message_count, 2);
    assert!(summaries[1].started_at > 0);
}

#[test]
fn active_path_walks_parents() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("a")).unwrap();
    writer.append_message(&Message::user_text("b")).unwrap();
    writer.append_message(&Message::user_text("c")).unwrap();

    let entries = load_session(writer.path()).unwrap();
    let path = active_path(&entries);
    assert_eq!(path.len(), 3);
    assert_eq!(path[0].seq, 0);
    assert_eq!(path[2].seq, 2);
}

#[test]
fn compaction_entry_roundtrip_and_replay() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("old 1")).unwrap();
    writer.append_message(&Message::user_text("old 2")).unwrap();
    let tip = writer.tip_id().unwrap().to_string();
    // 旧式落盘（无 keep_from_entry）：只保留摘要
    writer
        .append_compaction(&CompactionRecord {
            summary: "此前对话的摘要",
            strategy: "llm-summarize",
            keep_from_entry: "",
            source_tip: &tip,
            usage: None,
        })
        .unwrap();
    writer
        .append_message(&Message::user_text("new turn"))
        .unwrap();

    let entries = load_session(writer.path()).unwrap();
    // Compaction 条目随 load 原样往返（serde snake_case 兼容）
    assert!(matches!(
        &entries[2].kind,
        EntryKind::Compaction { summary, source_tip, strategy, keep_from_entry, .. }
            if summary == "此前对话的摘要"
                && source_tip == &tip
                && strategy == "llm-summarize"
                && keep_from_entry.is_empty()
    ));

    let active = active_path(&entries);
    let messages = rebuild_messages(&active);
    // 旧消息被摘要替换，摘要之后的新消息保留
    assert_eq!(messages.len(), 2);
    assert_eq!(summary_text(&messages[0]), Some("此前对话的摘要"));
    assert!(matches!(
        &messages[1],
        Message::User { content, .. } if content == "new turn"
    ));
}

/// 回归：`keep_from_entry` 起的那段原文必须活过重开 —— 内存里保留了什么，
/// 回放后就该是什么（此前回放只留摘要，重开后那段精确原文就丢了）。
#[test]
fn replay_keeps_messages_from_first_kept_entry() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer
        .append_message(&Message::user_text("turn 1"))
        .unwrap();
    writer
        .append_message(&Message::user_text("turn 2"))
        .unwrap();
    let kept = writer
        .append_message(&Message::user_text("turn 3"))
        .unwrap();
    writer
        .append_message(&Message::user_text("turn 4"))
        .unwrap();
    let tip = writer.tip_id().unwrap().to_string();
    // 内存历史：[1,2,3,4]，保留区间从 index 2（turn 3）开始
    assert_eq!(writer.message_ids().len(), 4);
    assert_eq!(writer.message_ids()[2], kept);
    writer
        .append_compaction(&CompactionRecord {
            summary: "前两轮摘要",
            strategy: "llm-summarize",
            keep_from_entry: &kept,
            source_tip: &tip,
            usage: None,
        })
        .unwrap();
    writer
        .append_message(&Message::user_text("turn 5"))
        .unwrap();

    let entries = load_session(writer.path()).unwrap();
    let active = active_path(&entries);
    let messages = rebuild_messages(&active);
    // 摘要 + 保留的 turn 3/4 + 压缩后的 turn 5
    assert_eq!(messages.len(), 4);
    assert_eq!(summary_text(&messages[0]), Some("前两轮摘要"));
    let texts: Vec<&str> = messages
        .iter()
        .skip(1)
        .map(|message| match message {
            Message::User { content, .. } => content.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(texts, vec!["turn 3", "turn 4", "turn 5"]);

    // 重开后 writer 的映射与回放结果一致（下一次压缩能继续换算 id）
    let reopened = SessionWriter::open(writer.path()).unwrap();
    assert_eq!(reopened.message_ids().len(), messages.len());
    assert_eq!(reopened.message_ids()[1], kept);
}

/// 定位不到 keep_from_entry（条目被分叉截断等）时退回「只留摘要」，
/// 不猜位置、不产生无主消息。
#[test]
fn replay_falls_back_to_summary_only_when_kept_entry_is_missing() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("a")).unwrap();
    let tip = writer.tip_id().unwrap().to_string();
    writer
        .append_compaction(&CompactionRecord {
            summary: "摘要",
            strategy: "llm-summarize",
            keep_from_entry: "不存在的条目 id",
            source_tip: &tip,
            usage: None,
        })
        .unwrap();
    let entries = load_session(writer.path()).unwrap();
    let messages = rebuild_messages(&active_path(&entries));
    assert_eq!(messages.len(), 1);
    assert_eq!(summary_text(&messages[0]), Some("摘要"));
}

/// 摘要正文的包裹与取回是同一套规则（落盘存原文，回放时包裹）。
#[test]
fn summary_message_roundtrip_is_not_nested() {
    let message = summary_message("## Goal\n做事");
    assert_eq!(summary_text(&message), Some("## Goal\n做事"));
    assert_eq!(summary_text(&Message::user_text("普通消息")), None);
    // 二次包裹仍能取回同一份正文（不会套娃）
    let again = summary_message(summary_text(&message).unwrap());
    assert_eq!(summary_text(&again), Some("## Goal\n做事"));
}

#[test]
fn compaction_entry_persists_summary_usage() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("a")).unwrap();
    let tip = writer.tip_id().unwrap().to_string();
    let usage = Usage {
        input: 40_000,
        output: 800,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 40_800,
    };
    writer
        .append_compaction(&CompactionRecord {
            summary: "摘要",
            strategy: "llm-summarize",
            keep_from_entry: "",
            source_tip: &tip,
            usage: Some(usage),
        })
        .unwrap();
    let entries = load_session(writer.path()).unwrap();
    assert!(matches!(
        &entries[1].kind,
        EntryKind::Compaction { usage: Some(recorded), .. } if *recorded == usage
    ));
}

#[test]
fn env_entry_roundtrip_idempotent_and_replay_neutral() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("first")).unwrap();

    let declared = vec![
        EnvDeclared {
            key: "AV_AGENT".into(),
            source: "runtime".into(),
        },
        EnvDeclared {
            key: "GITHUB_TOKEN".into(),
            source: "agent.local.toml".into(),
        },
    ];
    assert!(writer.append_env(declared.clone()).unwrap().is_some());
    // 幂等：同一会话再次记账不追加
    assert!(writer.append_env(declared.clone()).unwrap().is_none());
    writer
        .append_message(&Message::user_text("second"))
        .unwrap();

    let entries = load_session(writer.path()).unwrap();
    assert!(matches!(
        &entries[1].kind,
        EntryKind::Env { declared: recorded } if recorded == &declared
    ));
    // 打开旧会话：记账状态恢复，再调用仍幂等
    let mut reopened = SessionWriter::open(writer.path()).unwrap();
    assert!(reopened.append_env(declared).unwrap().is_none());

    // 回放中性：env 条目不影响消息流
    let active = active_path(&entries);
    let messages = rebuild_messages(&active);
    assert_eq!(messages.len(), 2);
    assert!(matches!(
        &messages[0],
        Message::User { content, .. } if content == "first"
    ));
    assert!(matches!(
        &messages[1],
        Message::User { content, .. } if content == "second"
    ));
}

#[test]
fn rebuild_without_compaction_keeps_all_messages() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("a")).unwrap();
    writer.append_custom("bookmark").unwrap();
    writer
        .append_model_change(&Model {
            id: "m".into(),
            name: "m".into(),
            api: pipi_protocol::Api::AnthropicMessages,
            base_url: String::new(),
            max_tokens: 4096,
            context_window: 100_000,
        })
        .unwrap();

    let entries = load_session(writer.path()).unwrap();
    let active = active_path(&entries);
    let messages = rebuild_messages(&active);
    assert_eq!(messages.len(), 1);
}

#[test]
fn archive_session_file_moves_and_refuses_conflicts() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("hi")).unwrap();
    let path = writer.path().to_path_buf();
    let id = path.file_stem().unwrap().to_str().unwrap().to_string();
    drop(writer);

    let archived = archive_session_file(&dir, &id).unwrap();
    assert!(!path.exists(), "原文件应当已经移走");
    assert!(archived.is_file(), "归档区应当有该文件");
    assert_eq!(
        archived.file_name().unwrap().to_str().unwrap(),
        format!("{id}.jsonl")
    );
    // 归档后活跃区查不到
    assert!(!dir.join(format!("{id}.jsonl")).exists());

    // 重复归档：活跃区已无此会话
    assert!(archive_session_file(&dir, &id).is_err());
    // 非法 id
    assert!(archive_session_file(&dir, "../evil").is_err());
    // 目标已存在时拒绝（把文件放回去再归档一次）
    std::fs::rename(&archived, &path).unwrap();
    std::fs::write(&archived, "{}\n").unwrap();
    let error = archive_session_file(&dir, &id).unwrap_err();
    assert!(error.contains("已存在"), "{error}");
}

#[test]
fn fork_marks_env_recorded_when_path_already_has_env_entry() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    writer.append_message(&Message::user_text("hi")).unwrap();
    writer
        .append_env(vec![EnvDeclared {
            key: "AV_AGENT".into(),
            source: "runtime".into(),
        }])
        .unwrap();
    let source_path = writer.path().to_path_buf();
    drop(writer);

    let mut forked = fork_session(&source_path, &dir, None).unwrap();
    // 活跃路径已带 Env 条目 → 分叉出的会话不应再记一条
    assert!(
        forked
            .append_env(vec![EnvDeclared {
                key: "AV_AGENT".into(),
                source: "runtime".into(),
            }])
            .unwrap()
            .is_none()
    );
}

#[test]
fn fork_session_copies_active_history() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    let id1 = writer.append_message(&Message::user_text("msg 1")).unwrap();
    let _id2 = writer.append_message(&Message::user_text("msg 2")).unwrap();
    let source_path = writer.path().to_path_buf();
    drop(writer);

    // 分叉到 id1
    let mut forked = fork_session(&source_path, &dir, Some(&id1)).unwrap();
    assert_eq!(forked.tip_id(), Some(id1.as_str()));
    let id3 = forked
        .append_message(&Message::user_text("msg 3 branch"))
        .unwrap();
    let forked_entries = load_session(forked.path()).unwrap();
    assert_eq!(forked_entries.len(), 2);
    assert_eq!(forked_entries[1].parent_id.as_deref(), Some(id1.as_str()));
    assert_eq!(forked_entries[1].id, id3);
}

#[test]
fn model_change_entry_roundtrip_and_active_resolution() {
    let dir = pipi_test_support::temp_dir("pipi-test");
    let mut writer = SessionWriter::create(&dir).unwrap();
    let model1 = Model {
        id: "gpt-4o".into(),
        name: "GPT-4o".into(),
        api: pipi_protocol::Api::OpenAICompletions,
        base_url: "https://api.openai.com/v1".into(),
        max_tokens: 4096,
        context_window: 128000,
    };
    let model2 = Model {
        id: "claude-sonnet-4-5".into(),
        name: "Claude Sonnet".into(),
        api: pipi_protocol::Api::AnthropicMessages,
        base_url: "https://api.anthropic.com".into(),
        max_tokens: 8192,
        context_window: 200000,
    };

    writer.append_message(&Message::user_text("hello")).unwrap();
    writer.append_model_change(&model1).unwrap();
    let id_mid = writer
        .append_message(&Message::user_text("with gpt-4o"))
        .unwrap();
    writer.append_model_change(&model2).unwrap();
    writer
        .append_message(&Message::user_text("with claude"))
        .unwrap();

    let entries = load_session(writer.path()).unwrap();
    assert_eq!(entries.len(), 5);

    // 最新活跃模型应为 model2
    let active = active_model_from_entries(&entries).unwrap();
    assert_eq!(active.id, "claude-sonnet-4-5");

    // 分叉到 id_mid，活跃模型应为 model1
    let forked = fork_session(writer.path(), &dir, Some(&id_mid)).unwrap();
    let forked_entries = load_session(forked.path()).unwrap();
    let forked_active = active_model_from_entries(&forked_entries).unwrap();
    assert_eq!(forked_active.id, "gpt-4o");

    let summaries = list_session_summaries(&dir);
    let current_summary = summaries
        .iter()
        .find(|s| s.id == writer.path().file_stem().unwrap().to_str().unwrap())
        .unwrap();
    assert_eq!(current_summary.model.as_deref(), Some("Claude Sonnet"));
}
