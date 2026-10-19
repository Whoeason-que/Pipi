//! forum-adversary-v2（代码向）：幂等正确性 / 并发 / 损坏恢复的对抗式复现。
//!
//! 只做演示，不改生产代码。某个测试失败 = 复现了一条真实缺陷；
//! 报告见 `docs/research/forum/review-v2-code.md`。
//!
//! 运行：`cargo test -p pipi-forum --test adversarial_v2_code`

use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use pipi_forum::FileForumRepo;
use pipi_protocol::forum::{ForumRepo, NewPost, NewThread, PostRange, PostVia};

fn open(prefix: &str) -> (FileForumRepo, PathBuf) {
    let root = pipi_test_support::temp_dir(prefix);
    let repo = FileForumRepo::open(root.clone()).unwrap();
    (repo, root)
}

fn thread_draft(author: &str, title: &str, body: &str) -> NewThread {
    NewThread {
        title: title.into(),
        author: author.into(),
        body: body.into(),
        tags: vec![],
        mentions: vec![],
        via: PostVia::Tool,
        origin: Some("test-run".into()),
    }
}

fn post_draft(author: &str, body: &str) -> NewPost {
    NewPost {
        author: author.into(),
        body: body.into(),
        via: PostVia::Tool,
        reply_to: None,
        mentions: vec![],
        origin: None,
    }
}

/// 带显式 origin 的回帖草稿（幂等只对「同一请求」生效）。
fn tagged_post(author: &str, body: &str, origin: &str) -> NewPost {
    NewPost {
        origin: Some(origin.into()),
        ..post_draft(author, body)
    }
}

// ---------------------------------------------------------------------------
// 发现 1（Medium）：建主题的幂等扫描（`find_duplicate_thread`）对「同作者 + 同
// 标题」的既有主题直接 `read_posts_in(&dir)?`。若这个既有主题的 `posts/` 变成了
// 符号链接（或普通文件 / 读不了的目录），扫描报错向上冒泡，`create_thread`
// 直接失败 —— 一条**损坏的已有主题**会让该 Agent 无法再建/重试任何同名主题。
//
// 幂等扫描是「尽力而为」的性质（读元数据失败已经 `continue` 跳过），但读帖子
// 失败却 hard-fail，两处策略不一致。
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn a_broken_existing_thread_aborts_the_idempotent_scan() {
    let (repo, root) = open("adv2-dup-broken");
    let first = repo
        .create_thread(&thread_draft("builder", "同一个标题", "原正文"))
        .unwrap();

    // 让同名既有主题的 posts/ 变成一个符号链接（工作区论坛可被 git/人改成这样）。
    let posts = root.join("threads").join(&first.id).join("posts");
    std::fs::remove_dir_all(&posts).unwrap();
    let outside = pipi_test_support::temp_dir("adv2-dup-broken-out");
    std::os::unix::fs::symlink(&outside, &posts).unwrap();

    // 重试（即便正文不同）应当能继续（最差也该返回已有主题），而不是 hard-fail。
    let retry = repo.create_thread(&thread_draft("builder", "同一个标题", "另一个正文"));
    assert!(
        retry.is_ok(),
        "幂等扫描因既有主题的 posts/ 损坏而失败，Agent 无法再建同名主题: {retry:?}"
    );
}

// ---------------------------------------------------------------------------
// 发现 2（Low–Medium，设计取舍）：幂等键 `(author, reply_to, content_hash)` 不含
// `origin`/run/时间，所以**同一作者在同一父帖下的两次合法相同发言**会被静默吞掉：
// 第二次 `append_post` 返回第一条帖（旧的 id/seq），工具对模型回报「已回帖 #1」，
// 模型以为发了，实际没有。
//
// 触发：跨 run 的合法重发（例如「已处理」在两次不同任务后各回一次），
// 或同一 run 内模型刻意重复同一句（replyTo 相同）。
// ---------------------------------------------------------------------------
#[test]
fn legitimate_identical_replies_are_silently_swallowed() {
    let (repo, _root) = open("adv2-identical-reply");
    let thread = repo
        .create_thread(&thread_draft("host", "标题", "根帖"))
        .unwrap();

    let first = repo
        .append_post(&thread.id, &post_draft("builder", "已处理"))
        .unwrap();
    let second = repo
        .append_post(&thread.id, &post_draft("builder", "已处理"))
        .unwrap();

    assert_ne!(
        first.id, second.id,
        "同一作者对同一父帖的第二次合法相同发言被幂等去重吞掉（返回旧的 #{}）",
        first.seq
    );
    assert_eq!(
        repo.posts(&thread.id, PostRange::default()).unwrap().len(),
        3,
        "根帖 + 两次发言应为 3 条"
    );
}

// ---------------------------------------------------------------------------
// （已修）`content_hash` 现在先归一化行尾再哈希；同一请求（同 origin）重试即使
// 行尾从 LF 变成 CRLF 也会命中幂等。人写（无 origin）不参与去重。
// ---------------------------------------------------------------------------
#[test]
fn retry_with_reformatted_line_endings_is_still_idempotent() {
    let (repo, _root) = open("adv2-crlf-dedup");
    let thread = repo
        .create_thread(&thread_draft("host", "标题", "根帖"))
        .unwrap();

    let first = repo
        .append_post(&thread.id, &tagged_post("builder", "已收到\n", "run:1"))
        .unwrap();
    let retry = repo
        .append_post(&thread.id, &tagged_post("builder", "已收到\r\n", "run:1"))
        .unwrap();

    assert_eq!(
        first.id, retry.id,
        "归一化行尾后，同一请求的重试应命中幂等，不重复发帖"
    );
}

// ---------------------------------------------------------------------------
// 自我证伪 / 探针（应当通过）：同进程内并发「同 key 建主题」与「同内容回帖」都由
// `write_lock` 串行化，幂等扫描与写入在同一临界区 —— 这条我**无法证伪**，记为正确。
// ---------------------------------------------------------------------------
#[test]
fn concurrent_identical_thread_creation_is_serialized() {
    let (repo, _root) = open("adv2-concurrent-thread");
    let repo = Arc::new(repo);
    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let mut handles = Vec::new();
    for _ in 0..workers {
        let repo = repo.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            repo.create_thread(&thread_draft("host", "并发同名", "同正文"))
                .unwrap()
                .id
        }));
    }
    let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let first = ids[0].clone();
    assert!(
        ids.iter().all(|id| id == &first),
        "并发同 key 建主题产生了多个主题: {ids:?}"
    );
}

#[test]
fn concurrent_identical_replies_are_serialized() {
    let (repo, _root) = open("adv2-concurrent-reply");
    let thread = repo
        .create_thread(&thread_draft("host", "标题", "根帖"))
        .unwrap();
    let id = thread.id.clone();
    let repo = Arc::new(repo);
    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let mut handles = Vec::new();
    for _ in 0..workers {
        let repo = repo.clone();
        let barrier = barrier.clone();
        let id = id.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            repo.append_post(&id, &tagged_post("builder", "同一条", "run:1"))
                .unwrap()
                .id
        }));
    }
    let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let first = ids[0].clone();
    assert!(
        ids.iter().all(|id| id == &first),
        "并发同一请求（同 origin）的回帖产生了多帖: {ids:?}"
    );
    assert_eq!(
        repo.posts(&id, PostRange::default()).unwrap().len(),
        2,
        "根帖 + 1 条去重后的回复"
    );
}
