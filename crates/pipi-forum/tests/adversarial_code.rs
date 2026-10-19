//! 对抗式审查（并发 / 原子性 / 崩溃一致性）的最小复现。
//!
//! 本文件**只做演示**，不修改任何生产代码。若某个测试失败，说明它复现了一个
//! 真实缺陷；报告见 `docs/research/forum/review-code.md`。
//!
//! 运行：`cargo test -p pipi-forum --test adversarial_code`

use std::path::PathBuf;

use pipi_forum::FileForumRepo;
use pipi_protocol::forum::{
    ForumRepo, NewPost, NewThread, PostRange, PostVia, SearchOptions, ThreadFilter, ThreadStatus,
};

fn open(prefix: &str) -> (FileForumRepo, PathBuf) {
    let root = pipi_test_support::temp_dir(prefix);
    let repo = FileForumRepo::open(root.clone()).unwrap();
    (repo, root)
}

fn thread_draft(author: &str, body: &str) -> NewThread {
    NewThread {
        title: "t".into(),
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

// ---------------------------------------------------------------------------
// 发现 A：`search` 在 Unicode 大小写折叠改变字节长度时 panic。
//
// `match_snippet` 用 `haystack = body.to_lowercase()` 上的**字节偏移** `at` 去切
// 原始 `body`：`&body[..at]`。`char::to_lowercase` 可能改变 UTF-8 字节长度
// （U+212A KELVIN SIGN 3 字节 → 'k' 1 字节），于是 `at` 落在原 body 的多字节字符
// 中间，触发 `byte index is not a char boundary` panic。
//
// 触发：正文含 `"\u{212A}x"`（U+212A KELVIN SIGN 3 字节），检索词 `x`。
// ---------------------------------------------------------------------------
#[test]
fn search_panics_on_unicode_lowercase_byte_length_change() {
    let (repo, _root) = open("pipi-forum-adv-search-panic");
    let body = "\u{212A}x"; // KELVIN SIGN + 'x'，to_lowercase 后变短 1 字节
    assert_eq!(body.len(), 4, "前置条件：U+212A 是 3 字节");
    repo.create_thread(&thread_draft("planner", body)).unwrap();

    // 只要命中，就走到 `body[..at]`。U+212A 小写后缩短，`at` 落在其字节中间 → panic。
    let hits = repo
        .search("x", SearchOptions::default())
        .expect("search 不应 panic");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains('x'));
}

// ---------------------------------------------------------------------------
// 发现 B：符号链接逃逸。设计文档声称「实现必须对符号链接与路径穿越
// fail-closed」，但 `FileForumRepo::open` 只对 root 调 `ensure_real_directory`，
// 对 `threads/`、`.archive/`、`<thread>/posts/` 一律不查。`locate` / `thread_ids_in`
// 用 `is_dir()`（跟随符号链接），写入用 `create_dir_all`（跟随符号链接）。
//
// 触发：把 `<root>/threads` 换成指向外部目录的符号链接。
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn symlinked_threads_dir_escapes_the_root() {
    let root = pipi_test_support::temp_dir("pipi-forum-adv-symlink-root");
    let outside = pipi_test_support::temp_dir("pipi-forum-adv-symlink-outside");
    let repo = FileForumRepo::open(root.clone()).unwrap();

    // 把活跃区整个换成指向外部的符号链接。
    std::os::unix::fs::symlink(&outside, root.join("threads")).unwrap();

    // 若 fail-closed，这里应当报错（或至少不写穿到 outside）。
    let created = repo.create_thread(&thread_draft("planner", "正文"));
    let escaped = std::fs::read_dir(&outside)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.path().is_dir());

    // fail-closed 属性：不得在 root 之外写入任何主题目录。
    assert!(
        !escaped,
        "符号链接逃逸：create_thread={:?}，外部目录 {} 下出现了主题目录",
        created.is_ok(),
        outside.display()
    );
}

// ---------------------------------------------------------------------------
// 发现 B'：读取方向同样逃逸 —— 外部目录里的合法主题能被 `thread()` 读到。
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn symlinked_threads_dir_reads_threads_outside_the_root() {
    let root = pipi_test_support::temp_dir("pipi-forum-adv-symlink-read-root");
    let outside = pipi_test_support::temp_dir("pipi-forum-adv-symlink-read-outside");
    let repo = FileForumRepo::open(root.clone()).unwrap();

    // 直接在外部目录造一个合法主题目录，再让 threads 指向它。
    let id = "1758000000000-ab12cd34";
    let dir = outside.join(id);
    std::fs::create_dir_all(dir.join("posts")).unwrap();
    std::fs::write(
        dir.join("thread.toml"),
        "id = \"1758000000000-ab12cd34\"\ntitle = \"secret\"\nauthor = \"root\"\ncreated = 1\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, root.join("threads")).unwrap();

    // fail-closed 属性：不应读到 root 之外的主题。
    assert!(
        repo.thread(id).is_err(),
        "符号链接逃逸：读到了 root 之外的主题元数据"
    );
}

// ---------------------------------------------------------------------------
// 发现 C：`set_status` / `set_pinned` 是对同一个 `thread.toml` 的
// read-modify-write，却**完全不取 `write_lock`**，也没有任何别的串行点。
// 两个并发调用会互相覆盖（lost update）：状态与置顶只能留下一个。
//
// 这是确定性竞态（不是推测）：两方法各自 read 整份 thread 再整体覆盖写。
// 本用例循环并发执行，第一次观察到「两者未同时生效」即失败。
// ---------------------------------------------------------------------------
#[test]
fn concurrent_set_status_and_pin_lose_updates() {
    for attempt in 0..200 {
        let (repo, _root) = open("pipi-forum-adv-lost-update");
        let thread = repo.create_thread(&thread_draft("host", "起帖")).unwrap();
        let id = thread.id.clone();
        let repo = std::sync::Arc::new(repo);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let r1 = repo.clone();
        let b1 = barrier.clone();
        let id1 = id.clone();
        let t1 = std::thread::spawn(move || {
            b1.wait();
            r1.set_status(&id1, ThreadStatus::Closed).unwrap();
        });
        let r2 = repo.clone();
        let b2 = barrier.clone();
        let id2 = id.clone();
        let t2 = std::thread::spawn(move || {
            b2.wait();
            r2.set_pinned(&id2, true).unwrap();
        });
        barrier.wait();
        t1.join().unwrap();
        t2.join().unwrap();

        let thread = repo.thread(&id).unwrap();
        if thread.status != ThreadStatus::Closed || !thread.pinned {
            panic!(
                "lost update（第 {attempt} 次）：并发 set_status(Closed)+set_pinned(true) 后 \
                 得到 status={:?} pinned={}，应两者都生效",
                thread.status, thread.pinned
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 发现 D：`archive` 与 `append_post` 之间没有互斥（append 持 `write_lock`，
// archive 不持）。archive 用 `fs::rename` 把主题目录搬走，而 append 里的
// `create_dir_all(threads/<id>/posts)` 会把被搬走的目录**重新创建**成一个
// 只有 posts 没有 thread.toml 的空壳：主题被撕裂成「归档区一份 + 活跃区空壳」，
// 之后 `thread()` 因活跃区存在而走进去、读不到 thread.toml 而报错。
//
// 这是压力复现：反复并发 archive + append，只要观察到活跃区空壳即失败。
// ---------------------------------------------------------------------------
#[test]
fn archive_racing_append_can_split_the_thread_directory() {
    let attempts = 300;
    for attempt in 0..attempts {
        let (repo, root) = open("pipi-forum-adv-archive-race");
        let thread = repo.create_thread(&thread_draft("host", "起帖")).unwrap();
        let id = thread.id.clone();
        let repo = std::sync::Arc::new(repo);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let r1 = repo.clone();
        let b1 = barrier.clone();
        let id1 = id.clone();
        let t1 = std::thread::spawn(move || {
            b1.wait();
            let _ = r1.append_post(&id1, &post_draft("writer", "并发回帖"));
        });
        let r2 = repo.clone();
        let b2 = barrier.clone();
        let id2 = id.clone();
        let t2 = std::thread::spawn(move || {
            b2.wait();
            let _ = r2.archive(&id2);
        });
        barrier.wait();
        t1.join().unwrap();
        t2.join().unwrap();

        let active = root.join("threads").join(&id);
        let archived = root.join(".archive").join(&id);
        // 撕裂：归档区已有主题，活跃区却还存在一个没有 thread.toml 的目录。
        if archived.is_dir() && active.is_dir() && !active.join("thread.toml").is_file() {
            panic!(
                "archive/append 竞态撕裂目录（第 {attempt} 次）：{} 与 {} 同时存在，\
                 活跃区缺 thread.toml；thread() 现在返回 {:?}",
                active.display(),
                archived.display(),
                repo.thread(&id).map(|t| t.title)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 发现 E（推测 / 无法在本进程复现）：跨进程同毫秒同序号。
// `append_post_in` 只靠 `create_new` 的名字冲突兜底，而文件名含随机 id，
// 两个进程对同一主题各自 `read_posts_in` 得到相同 max seq 后，会写出
// 同一个 seq、不同 id 的两条帖子（文件名不冲突，create_new 不报 AlreadyExists）。
// 设计文档已承认「跨进程无写者锁」，故此处只记录，不写用例。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 发现 F：帖子目录里一个「读不出来」的文件会让**整个主题**不可读。
//
// `read_posts_in` 对 `decode_post` 的失败选择跳过（坏行策略），但对
// `fs::read_to_string(...)?` 的**读失败直接 `?` 冒泡**：非 UTF-8（崩溃时写一半、
// 切断多字节字符的典型残留）、或被人/其他进程放了一个同名目录，都会让
// `posts()` / `thread()` / `append_post` 全部失败 —— 单文件坏 → 主题被 brick。
//
// 触发：在 posts/ 下制造一个非法 UTF-8 的 *.md 文件。
// ---------------------------------------------------------------------------
#[test]
fn unreadable_post_file_bricks_the_whole_thread() {
    let (repo, root) = open("pipi-forum-adv-brick");
    let thread = repo.create_thread(&thread_draft("host", "起帖")).unwrap();
    let posts_dir = root.join("threads").join(&thread.id).join("posts");

    // 模拟崩溃时写一半、在多字节字符中间截断的残留（非法 UTF-8）。
    std::fs::write(posts_dir.join("0002-deadbeef.md"), [0xff, 0xfe, 0x00]).unwrap();

    // 一条坏文件不应让整个主题读不出来（对比 decode 失败被跳过的坏行策略）。
    let posts = repo.posts(&thread.id, PostRange::default());
    assert!(
        posts.is_ok(),
        "一条非法 UTF-8 的帖子文件把整个主题 brick 了: {:?}",
        posts.err()
    );
    assert!(repo.thread(&thread.id).is_ok(), "thread() 也被 brick");
    assert!(
        repo.append_post(&thread.id, &post_draft("w", "还能回帖吗"))
            .is_ok(),
        "append_post 也被 brick"
    );
}

// 同类：posts/ 下一个名为 `*.md` 的**目录**同样 brick 主题（read_to_string 报 Is a directory）。
#[test]
fn directory_named_md_bricks_the_whole_thread() {
    let (repo, root) = open("pipi-forum-adv-brick-dir");
    let thread = repo.create_thread(&thread_draft("host", "起帖")).unwrap();
    std::fs::create_dir_all(
        root.join("threads")
            .join(&thread.id)
            .join("posts")
            .join("0002-deadbeef.md"),
    )
    .unwrap();
    let posts = repo.posts(&thread.id, PostRange::default());
    assert!(
        posts.is_ok(),
        "一个名为 *.md 的目录把整个主题 brick 了: {:?}",
        posts.err()
    );
}

// ---------------------------------------------------------------------------
// 发现 G（已修）：`append_post` 的「已关闭」检查曾发生在**取锁之前**，且
// `set_status` 根本不取锁 —— 于是存在「回帖被接受进已关闭主题」的窗口。
//
// 现在两者共用同一把写锁，「检查→写入」在同一临界区内。
//
// 注意：并发跑 `set_status(Closed)` 与 `append_post` 时，最终状态**无法**区分两种
// 都合法的线性化（先关后拒 / 先回后关），所以原先「closed 且 count==2 ⇒ bug」的
// 断言本身不成立。这里改为断言真正的不变量：关帖一旦完成，其后的回帖必然被拒；
// 并发压力只检查不出现额外帖子（count ∈ {1,2}）。
// ---------------------------------------------------------------------------
#[test]
fn closed_thread_rejects_posts_and_stays_consistent_under_race() {
    for attempt in 0..200 {
        let (repo, _root) = open("pipi-forum-adv-close-race");
        let thread = repo.create_thread(&thread_draft("host", "起帖")).unwrap();
        let id = thread.id.clone();
        let repo = std::sync::Arc::new(repo);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let r1 = repo.clone();
        let b1 = barrier.clone();
        let id1 = id.clone();
        let t1 = std::thread::spawn(move || {
            b1.wait();
            r1.set_status(&id1, ThreadStatus::Closed).unwrap();
        });
        let r2 = repo.clone();
        let b2 = barrier.clone();
        let id2 = id.clone();
        let t2 = std::thread::spawn(move || {
            b2.wait();
            let _ = r2.append_post(&id2, &post_draft("writer", "并发回帖"));
        });
        barrier.wait();
        t1.join().unwrap();
        t2.join().unwrap();

        let count = repo.posts(&id, PostRange::default()).unwrap().len();
        assert!(
            count == 1 || count == 2,
            "第 {attempt} 次：帖数异常 {count}（既不是 1 也不是 2）"
        );
        // 关帖已完成，之后的回帖必须失败。
        assert!(
            repo.append_post(&id, &post_draft("late", "迟到回帖"))
                .is_err(),
            "第 {attempt} 次：已关闭的主题仍接受回帖"
        );
    }
}

// 让 `PostRange` / `ThreadFilter` 的 import 总被用到（避免 unused 警告影响阅读）。
#[test]
fn probe_defaults_are_reachable() {
    let (repo, _root) = open("pipi-forum-adv-probe");
    let thread = repo.create_thread(&thread_draft("host", "x")).unwrap();
    let _ = repo.posts(&thread.id, PostRange::default()).unwrap();
    let _ = repo.list_threads(&ThreadFilter::default(), 10).unwrap();
}
