//! 论坛载体的黑盒回归：整条生命周期（建/回/读/检索/关闭/归档/恢复）、磁盘布局，
//! 以及「文件即真相」——人工直接编辑帖子文件后仍能被读回。

use std::path::PathBuf;

use pipi_forum::FileForumRepo;
use pipi_protocol::forum::{
    ForumRepo, NewPost, NewThread, PostRange, PostVia, SearchOptions, ThreadFilter, ThreadStatus,
};

fn open() -> (FileForumRepo, PathBuf) {
    let root = pipi_test_support::temp_dir("pipi-forum-lifecycle");
    let repo = FileForumRepo::open(root.clone()).unwrap();
    (repo, root)
}

fn draft(author: &str, body: &str) -> NewPost {
    NewPost {
        author: author.into(),
        body: body.into(),
        via: PostVia::Tool,
        reply_to: None,
        mentions: vec![],
        origin: None,
    }
}

/// 主题目录下第一个帖子文件的路径（`posts/0001-*.md`）。
fn first_post_path(root: &std::path::Path, thread_id: &str) -> PathBuf {
    let posts_dir = root.join("threads").join(thread_id).join("posts");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&posts_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 1, "应有且仅有一条首帖");
    paths.pop().unwrap()
}

#[test]
fn full_lifecycle_keeps_files_human_readable_and_editable() {
    let (repo, root) = open();
    let thread = repo
        .create_thread(&NewThread {
            title: "Agent 之间怎么异步讨论".into(),
            author: "planner".into(),
            body: "起帖：我建议用文件存帖。".into(),
            tags: vec!["design".into()],
            mentions: vec!["builder".into()],
            via: PostVia::Tool,
            origin: Some("test-run".into()),
        })
        .unwrap();

    // 磁盘布局：thread.toml + posts/<seq>-<id>.md
    let post_path = first_post_path(&root, &thread.id);
    assert!(
        root.join("threads")
            .join(&thread.id)
            .join("thread.toml")
            .is_file()
    );
    assert!(
        post_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("00000001-")
    );
    let content = std::fs::read_to_string(&post_path).unwrap();
    assert!(
        content.starts_with("+++\n"),
        "帖子应以 TOML frontmatter 开头"
    );
    assert!(content.contains("author = \"planner\""));
    assert!(content.contains("起帖：我建议用文件存帖。"));

    // 人工直接编辑正文：元数据不变、正文读回。
    let edited = content.replace(
        "起帖：我建议用文件存帖。",
        "起帖：我建议用文件存帖。\n\n（人工补充：注意并发写。）",
    );
    std::fs::write(&post_path, edited).unwrap();
    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    assert_eq!(posts[0].author, "planner");
    assert!(posts[0].body.contains("人工补充"));

    // 回帖并指向首帖。
    let reply = repo
        .append_post(
            &thread.id,
            &NewPost {
                author: "builder".into(),
                body: "同意，我加一个回复。".into(),
                via: PostVia::Tool,
                reply_to: Some(posts[0].id.clone()),
                mentions: vec!["planner".into()],
                origin: None,
            },
        )
        .unwrap();
    assert_eq!(reply.seq, 2);
    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[1].reply_to.as_deref(), Some(posts[0].id.as_str()));
    assert!(posts[1].mentions_agent("planner"));

    // 检索命中「并发」。
    let hits = repo.search("并发", SearchOptions::default()).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].thread_id, thread.id);

    // 关闭后拒绝回帖。
    repo.set_status(&thread.id, ThreadStatus::Closed).unwrap();
    assert!(
        repo.append_post(&thread.id, &draft("carol", "还能回吗"))
            .is_err()
    );

    // 归档：活跃列表为空，但帖子仍可读；恢复后回到活跃列表。
    repo.archive(&thread.id).unwrap();
    assert!(
        repo.list_threads(&ThreadFilter::default(), 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.posts(&thread.id, PostRange::default()).unwrap().len(),
        2
    );
    repo.restore(&thread.id).unwrap();
    assert_eq!(
        repo.list_threads(&ThreadFilter::default(), 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn list_filters_sorts_and_paginates() {
    let (repo, _root) = open();
    let a = repo
        .create_thread(&NewThread {
            title: "A".into(),
            author: "a".into(),
            body: "aaa".into(),
            tags: vec!["x".into()],
            mentions: vec![],
            via: PostVia::Cli,
            origin: Some("test-run".into()),
        })
        .unwrap();
    let b = repo
        .create_thread(&NewThread {
            title: "B".into(),
            author: "b".into(),
            body: "bbb".into(),
            tags: vec!["y".into()],
            mentions: vec![],
            via: PostVia::Cli,
            origin: Some("test-run".into()),
        })
        .unwrap();
    repo.append_post(&b.id, &draft("b", "bbb2")).unwrap();

    // 置顶优先。
    repo.set_pinned(&a.id, true).unwrap();
    let list = repo.list_threads(&ThreadFilter::default(), 10).unwrap();
    assert_eq!(list[0].thread.id, a.id);
    assert_eq!(list[1].thread.id, b.id);
    assert_eq!(list[1].post_count, 2);

    // 标签过滤。
    let tagged = repo
        .list_threads(
            &ThreadFilter {
                tag: Some("y".into()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(tagged.len(), 1);
    assert_eq!(tagged[0].thread.id, b.id);

    // 分页：offset 跳过首帖。
    let page = repo
        .posts(
            &b.id,
            PostRange {
                offset: 1,
                limit: Some(10),
            },
        )
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].seq, 2);
}

#[test]
fn concurrent_appends_get_distinct_sequences() {
    use std::sync::Arc;
    let (repo, _root) = open();
    let thread = repo
        .create_thread(&NewThread {
            title: "并发发帖".into(),
            author: "host".into(),
            body: "起帖".into(),
            tags: vec![],
            mentions: vec![],
            via: PostVia::Tool,
            origin: Some("test-run".into()),
        })
        .unwrap();
    let repo = Arc::new(repo);
    let mut handles = Vec::new();
    for index in 0..8 {
        let repo = repo.clone();
        let thread_id = thread.id.clone();
        handles.push(std::thread::spawn(move || {
            repo.append_post(&thread_id, &draft("writer", &format!("帖 {index}")))
                .unwrap();
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    // 首帖 + 8 条并发回复，序号 1..=9 各不相同。
    assert_eq!(posts.len(), 9);
    let seqs: Vec<u64> = posts.iter().map(|post| post.seq).collect();
    let mut unique = seqs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), seqs.len(), "序号不得重复: {seqs:?}");
}
