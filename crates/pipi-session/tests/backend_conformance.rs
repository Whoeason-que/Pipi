//! 载体一致性：同一套场景与断言跑在**全部**后端上。
//!
//! 抄 pi `session-backends` 的 conformance 做法：端口（`SessionRepo` /
//! `SessionLog`）的语义不写在文档里、写在断言里 —— 新增载体时把
//! [`run_conformance`] 再跑一遍即可，不用重写一批用例。
//!
//! 只用公开入口（黑盒）：契约层类型 + `pipi_session` 的两个实现。

use pipi_protocol::Message;
use pipi_protocol::session::{
    CompactionRecord, EnvDeclared, SessionRepo, active_path, rebuild_messages, summary_text,
};
use pipi_session::{GitSessionRepo, JsonlSessionRepo};

fn run_conformance(repo: &dyn SessionRepo, label: &str) {
    let at = |what: &str| format!("[{label}] {what}");

    // 1. 新建 + 追加 + 提交：身份、tip、message_ids
    let mut sink = repo.create().unwrap_or_else(|e| panic!("{}", at(&e)));
    let id = sink.session_id().to_string();
    assert!(!sink.is_temporary(), "{}", at("正式会话不是临时的"));
    assert!(sink.tip_id().is_none(), "{}", at("新会话没有 tip"));
    let m1 = sink
        .append_message(&Message::user_text("第一轮：读一下 README"))
        .unwrap();
    let m2 = sink
        .append_message(&Message::assistant_text("好的", "mock"))
        .unwrap();
    let c1 = sink.append_custom("bookmark").unwrap();
    sink.commit_point().unwrap();
    assert_eq!(
        sink.tip_id(),
        Some(c1.as_str()),
        "{}",
        at("tip 指向最后一条")
    );
    assert_eq!(
        sink.message_ids(),
        &[m1.clone(), m2.clone()],
        "{}",
        at("custom 不进消息序列")
    );
    drop(sink);

    // 2. 目录级读回：条目、顺序、parent 链
    assert!(repo.exists(&id).unwrap(), "{}", at("会话存在"));
    let entries = repo.entries(&id).unwrap();
    assert_eq!(entries.len(), 3, "{}", at("三条条目"));
    assert_eq!(entries[0].seq, 0, "{}", at("seq 从 0 起"));
    assert!(entries[0].parent_id.is_none(), "{}", at("首条无父"));
    assert_eq!(
        entries[1].parent_id.as_deref(),
        Some(m1.as_str()),
        "{}",
        at("parent 链")
    );
    assert_eq!(
        entries[2].parent_id.as_deref(),
        Some(m2.as_str()),
        "{}",
        at("parent 链")
    );
    assert_eq!(entries[2].id, c1, "{}", at("custom 条目 id"));

    // 3. 重开续写：tip/seq 接得上，env 记账幂等
    let mut resumed = repo.open(&id).unwrap();
    assert_eq!(resumed.tip_id(), Some(c1.as_str()), "{}", at("重开后 tip"));
    assert_eq!(
        resumed.message_ids(),
        &[m1.clone(), m2.clone()],
        "{}",
        at("重开后消息映射")
    );
    let declared = vec![EnvDeclared {
        key: "AV_AGENT".into(),
        source: "runtime".into(),
    }];
    assert!(
        resumed.append_env(declared.clone()).unwrap().is_some(),
        "{}",
        at("首次记账写入")
    );
    assert!(
        resumed.append_env(declared.clone()).unwrap().is_none(),
        "{}",
        at("同一会话重复记账幂等")
    );
    let m3 = resumed
        .append_message(&Message::user_text("第二轮"))
        .unwrap();
    resumed.commit_point().unwrap();
    drop(resumed);

    let entries = repo.entries(&id).unwrap();
    assert_eq!(entries.len(), 5, "{}", at("user+assistant+custom+env+user"));
    assert_eq!(entries[4].id, m3, "{}", at("新条目是第二条 user"));
    assert_eq!(entries[4].seq, 4, "{}", at("seq 连续"));
    assert_eq!(
        entries[4].parent_id.as_deref(),
        Some(entries[3].id.as_str()),
        "{}",
        at("新条目挂在 env 之后")
    );

    // 重开后 env 幂等仍成立（记账状态随会话恢复）
    let mut reopened = repo.open(&id).unwrap();
    assert!(
        reopened.append_env(declared).unwrap().is_none(),
        "{}",
        at("重开后仍幂等")
    );
    drop(reopened);

    // 4. 压缩条目：keep_from_entry 保住的原文活过重开
    let mut sink = repo.open(&id).unwrap();
    sink.append_compaction(&CompactionRecord {
        summary: "前两轮摘要",
        strategy: "llm-summarize",
        keep_from_entry: &m2,
        source_tip: &m3,
        usage: None,
    })
    .unwrap();
    sink.commit_point().unwrap();
    drop(sink);

    let entries = repo.entries(&id).unwrap();
    let messages = rebuild_messages(&active_path(&entries));
    assert_eq!(
        summary_text(&messages[0]),
        Some("前两轮摘要"),
        "{}",
        at("回放以摘要开头")
    );
    assert!(messages.len() >= 3, "{}", at("保留段原文还在"));
    let reopened = repo.open(&id).unwrap();
    assert_eq!(
        reopened.message_ids().len(),
        messages.len(),
        "{}",
        at("重开后映射与回放一致（下一次压缩靠它换算）")
    );
    drop(reopened);

    // 5. 列表四字段
    let summaries = repo.list();
    let summary = summaries
        .iter()
        .find(|summary| summary.id == id)
        .unwrap_or_else(|| panic!("{}", at("列表里有这条会话")));
    assert_eq!(
        summary.title,
        "第一轮：读一下 README",
        "{}",
        at("标题取首个用户消息")
    );
    assert_eq!(summary.message_count, 3, "{}", at("消息条数"));
    assert!(summary.started_at > 0, "{}", at("started_at 来自 id 前缀"));
    assert_eq!(
        summary.last_active,
        entries.last().unwrap().timestamp,
        "{}",
        at("last_active 取最后一条的时间戳")
    );
    assert_eq!(
        summary.model.as_deref(),
        Some("mock"),
        "{}",
        at("模型回退到 assistant 消息")
    );

    // 6. 整线分叉：新 id、内容相同、两条线互不影响
    let forked = repo.fork(&id, None).unwrap();
    let forked_id = forked.session_id().to_string();
    assert_ne!(forked_id, id, "{}", at("分叉出新 id"));
    assert_eq!(
        forked.tip_id(),
        entries.last().map(|entry| entry.id.as_str()),
        "{}",
        at("分叉 tip == 源 tip")
    );
    drop(forked);
    assert_eq!(
        repo.entries(&forked_id).unwrap().len(),
        entries.len(),
        "{}",
        at("分叉内容与源一致")
    );
    let mut branch = repo.open(&forked_id).unwrap();
    let extra = branch
        .append_message(&Message::user_text("分叉后的新消息"))
        .unwrap();
    branch.commit_point().unwrap();
    drop(branch);
    let branch_entries = repo.entries(&forked_id).unwrap();
    assert_eq!(
        branch_entries.len(),
        entries.len() + 1,
        "{}",
        at("分叉线增长")
    );
    assert_eq!(branch_entries.last().unwrap().id, extra, "{}", at("新条目"));
    assert_eq!(
        repo.entries(&id).unwrap().len(),
        entries.len(),
        "{}",
        at("原线不受影响")
    );

    // 7. entry 级分叉：只带目标条目及其前缀
    let prefix = repo.fork(&id, Some(&m2)).unwrap();
    let prefix_id = prefix.session_id().to_string();
    assert_eq!(prefix.tip_id(), Some(m2.as_str()), "{}", at("分叉到 m2"));
    drop(prefix);
    let prefix_entries = repo.entries(&prefix_id).unwrap();
    assert_eq!(prefix_entries.len(), 2, "{}", at("只带 m1+m2"));
    assert_eq!(prefix_entries[1].id, m2, "{}", at("末条是 m2"));

    // 8. 归档 / 恢复 / 删除（活跃区与归档区各一套）
    repo.archive(&forked_id).unwrap();
    assert!(
        !repo.exists(&forked_id).unwrap(),
        "{}",
        at("归档后离开活跃区")
    );
    assert!(
        repo.list().iter().all(|summary| summary.id != forked_id),
        "{}",
        at("活跃列表里没有它")
    );
    assert!(
        repo.list_archived()
            .unwrap()
            .iter()
            .any(|summary| summary.id == forked_id),
        "{}",
        at("归档列表里有它")
    );
    repo.restore(&forked_id).unwrap();
    assert!(repo.exists(&forked_id).unwrap(), "{}", at("恢复回活跃区"));
    repo.delete(&forked_id).unwrap();
    assert!(!repo.exists(&forked_id).unwrap(), "{}", at("删除后不存在"));
    assert!(
        repo.entries(&forked_id).unwrap_err().contains("不存在"),
        "{}",
        at("读已删除会话报「不存在」")
    );
    repo.archive(&prefix_id).unwrap();
    repo.delete_archived(&prefix_id).unwrap();
    assert!(
        repo.list_archived()
            .unwrap()
            .iter()
            .all(|summary| summary.id != prefix_id),
        "{}",
        at("归档区删除生效")
    );

    // 9. 缺失 id 与非法 id
    assert!(
        !repo.exists("missing").unwrap(),
        "{}",
        at("缺失 id 是「不存在」")
    );
    assert!(
        repo.exists("../etc/passwd")
            .unwrap_err()
            .contains("非法会话 ID"),
        "{}",
        at("路径字符的 id 一律拒绝")
    );
}

#[test]
fn jsonl_backend_satisfies_the_conformance_suite() {
    let dir = pipi_test_support::temp_dir("pipi-conformance-jsonl");
    let repo = JsonlSessionRepo::new(dir.clone());
    run_conformance(&repo, "jsonl");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_backend_satisfies_the_conformance_suite() {
    let dir = pipi_test_support::temp_dir("pipi-conformance-git");
    let repo = GitSessionRepo::new(dir.join("sessions.git"));
    run_conformance(&repo, "git");
    std::fs::remove_dir_all(&dir).ok();
}
