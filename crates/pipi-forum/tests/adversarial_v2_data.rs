//! 对抗式数据审查（第二轮）：`content_hash` / `origin` 新字段、幂等去重、
//! 格式演进与手写文件的边界。
//!
//! 本文件**只做演示**，不修改任何生产代码。用例断言的是「应当成立的性质」：
//! 某个测试失败，说明它复现了一个真实缺陷（或一处文档与实现不一致）。
//! 报告见 `docs/research/forum/review-v2-data.md`。
//!
//! 运行：`cargo test -p pipi-forum --test adversarial_v2_data`
//!
//! 集成测试只能走公开入口，因此这里通过 `FileForumRepo` 读写**手工构造的磁盘
//! 文件**来接触编解码边界 —— 这正是「磁盘上的东西迟早会被手改坏 / 跨版本读」的
//! 对抗场景。

use std::fs;
use std::path::{Path, PathBuf};

use pipi_forum::FileForumRepo;
use pipi_protocol::forum::{ForumRepo, NewPost, NewThread, PostRange, PostVia, ThreadFilter};

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

fn post_draft(author: &str, body: &str, reply_to: Option<&str>) -> NewPost {
    NewPost {
        author: author.into(),
        body: body.into(),
        via: PostVia::Tool,
        reply_to: reply_to.map(str::to_string),
        mentions: vec![],
        origin: None,
    }
}

/// 带显式 origin 的回帖草稿（幂等只对「同一请求」生效）。
fn tagged_post(author: &str, body: &str, reply_to: &str, origin: &str) -> NewPost {
    NewPost {
        origin: Some(origin.into()),
        ..post_draft(author, body, Some(reply_to))
    }
}

fn posts_dir(root: &Path, id: &str) -> PathBuf {
    let dir = root.join("threads").join(id).join("posts");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn first_post_id(repo: &dyn ForumRepo, thread_id: &str) -> String {
    repo.posts(thread_id, PostRange::default()).unwrap()[0]
        .id
        .clone()
}

// ===========================================================================
// F1（高）: 新字段类型不符会静默丢掉整条帖子（数据丢失，non-fail-closed）
//
// `PostFrontmatter` 用强类型 `Option<String>` / `u32` 解码 `contentHash` /
// `origin` / `rootId` / `causalityDepth`。任何类型不符（手写、其他工具生成、
// 未来版本把标量升级成结构化值、编辑器把数字加引号）都会让 `toml::from_str`
// 整体失败 → `decode_post` 返回 Err → `read_posts_in` **静默跳过**该文件。
//
// 对比：未知**枚举值**有 `#[serde(other)]` 兜底、缺字段有 `#[serde(default)]`,
// 但类型不符没有任何容错 —— 一个坏字段 = 整条帖消失（post_count 少计、
// 因果链断裂、`last_author` 漂移），且没有任何告警。
//
// 期望：类型不符时应尽量降级（丢弃该字段或按默认），而不是丢掉整条帖子。
// ===========================================================================
#[test]
fn wrong_typed_new_field_does_not_drop_the_post() {
    let (repo, root) = open("pipi-forum-v2-wrongtype");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();
    let dir = posts_dir(&root, &thread.id);

    // contentHash 写成整数（手写/其他工具常见：忘了引号）。
    fs::write(
        dir.join("0002-evilhash.md"),
        "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\nvia = \"tool\"\ncontentHash = 123\n+++\n\n正文\n",
    )
    .unwrap();
    // origin 写成数组。
    fs::write(
        dir.join("0003-evilorigin.md"),
        "+++\nid = \"a-3\"\nseq = 3\nauthor = \"c\"\ncreated = 30\nvia = \"tool\"\norigin = [\"run:1\"]\n+++\n\n正文\n",
    )
    .unwrap();
    // causalityDepth 写成负数（u32 溢出）。
    fs::write(
        dir.join("0004-evildepth.md"),
        "+++\nid = \"a-4\"\nseq = 4\nauthor = \"d\"\ncreated = 40\nvia = \"tool\"\ncausalityDepth = -1\n+++\n\n正文\n",
    )
    .unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let seqs: Vec<u64> = posts.iter().map(|p| p.seq).collect();
    assert!(
        seqs.contains(&2) && seqs.contains(&3) && seqs.contains(&4),
        "新字段类型不符把整条帖子静默丢弃了（读到的 seq = {seqs:?}）"
    );
}

// ===========================================================================
// F2（中低）: 手写 snake_case 新字段被静默忽略（`reply_to` 的修法没有推广到新字段）
//
// 设计文档 §4 的 DTO 用 snake_case（`content_hash` / `root_id` /
// `causality_depth`），而 `PostFrontmatter` 标了 `rename_all = "camelCase"`，
// 磁盘键实际是 `contentHash` / `rootId` / `causalityDepth`。第一轮 review 只给
// `reply_to` 加了 `alias`（见 adversarial_data.rs 用例 4），本轮的三个新字段没有。
//
// 期望：与 `reply_to` 同款容错，手写 snake_case 能被识别。
// ===========================================================================
#[test]
fn snake_case_new_fields_are_honored_on_read() {
    let (repo, root) = open("pipi-forum-v2-snakeaware");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();
    let dir = posts_dir(&root, &thread.id);

    fs::write(
        dir.join("0002-snake.md"),
        "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\nvia = \"tool\"\n\
         content_hash = \"sha256:deadbeef\"\norigin = \"run:7\"\n\
         root_id = \"a-1\"\ncausality_depth = 3\n+++\n\n正文\n",
    )
    .unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let post = posts.iter().find(|p| p.seq == 2).expect("帖子应可读");
    assert_eq!(
        post.content_hash.as_deref(),
        Some("sha256:deadbeef"),
        "手写 content_hash 被静默忽略"
    );
    assert_eq!(post.origin.as_deref(), Some("run:7"), "手写 origin 被忽略");
    assert_eq!(post.root_id.as_deref(), Some("a-1"), "手写 root_id 被忽略");
    assert_eq!(post.causality_depth, 3, "手写 causality_depth 被忽略");
}

// ===========================================================================
// F3（中）: 幂等去重对「没有 contentHash 的旧帖」失效 → 重试产生重复帖
//
// 设计文档 §4/§6 承诺 `(author, reply_to, content_hash)` 幂等。但 `content_hash`
// 是 P1 才落盘的新字段；升级前写入的帖子（或手写、其他工具生成的帖子）只有
// `content_hash = None`。`append_post_in` 的去重条件要求
// `post.content_hash.as_deref() == Some(hash)`，None 永不等 → 同一条重试会**再发一遍**。
//
// 期望：同一请求（同 origin）的旧帖（无 contentHash）重试时，回退到比较正文，
// 不把「作者 + 父帖 + 正文逐字节相同」的重试当成新帖。
// ===========================================================================
#[test]
fn dedup_still_recognizes_posts_written_before_content_hash_existed() {
    let (repo, root) = open("pipi-forum-v2-old-dedup");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();
    let root_post = first_post_id(&repo, &thread.id);
    let first = repo
        .append_post(
            &thread.id,
            &tagged_post("b", "同一条正文", &root_post, "run:1"),
        )
        .unwrap();

    // 模拟旧版本写入的帖子：把 contentHash 行从文件里去掉（旧格式没有这个字段）。
    let path = root
        .join("threads")
        .join(&thread.id)
        .join("posts")
        .join(format!("{:08}-{}.md", first.seq, first.id));
    let content = fs::read_to_string(&path).unwrap();
    let stripped = content
        .lines()
        .filter(|line| !line.starts_with("contentHash"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !stripped.contains("contentHash"),
        "fixture 里未找到 contentHash 行"
    );
    fs::write(&path, stripped).unwrap();

    // 现在重试同一条（工具重放，同一 origin）：
    let again = repo
        .append_post(
            &thread.id,
            &tagged_post("b", "同一条正文", &root_post, "run:1"),
        )
        .unwrap();
    assert_eq!(
        first.id, again.id,
        "旧帖（无 contentHash）在同一请求重试时未回退到正文比较，产生了重复帖（新 id {}）",
        again.id
    );
}

// ===========================================================================
// F4（中）: 建主题的幂等键 `(author, title, 首帖 hash)` 会吞掉合法重复，
//          且忽略 tags/via/mentions —— 第二次创建的 tags 被静默丢弃，
//          调用方拿到的是旧主题。
//
// `find_duplicate_thread` 只用 author/title/正文哈希比较；tags 不同也判重复。
// 于是「同名 + 同正文 + 想换一组标签」的第二条命令不会生效，返回旧主题
// （旧标签），调用方（工具）还会打印「已创建主题」。
//
// 期望：参与幂等判定以外的输入（tags）不同时不应判为同一条；或至少返回错误/
// 明确告知调用方命中了已有主题。
// ===========================================================================
#[test]
fn thread_dedup_does_not_discard_a_new_tag_set() {
    let (repo, _root) = open("pipi-forum-v2-thread-tags");
    let mut first = thread_draft("a", "同名", "同正文");
    first.tags = vec!["alpha".into()];
    let created = repo.create_thread(&first).unwrap();

    let mut second = thread_draft("a", "同名", "同正文");
    second.tags = vec!["beta".into()];
    let again = repo.create_thread(&second).unwrap();

    // 期望：这是一个与首帖 tags 不同的新请求，不应被当成重放而吞掉。
    assert!(
        !(again.id == created.id && again.tags == vec!["alpha".to_string()]),
        "第二次创建（tags=[beta]）被当成重放：返回旧主题 {} tags={:?}，新 tags 被丢弃",
        again.id,
        again.tags
    );
}

// ===========================================================================
// F5（中低）: 回帖幂等键忽略 mentions，显式新提及被静默丢弃
//
// 去重条件只看 `(author, reply_to, content_hash)`。正文相同、父帖相同、作者相同，
// 但这次显式 `@` 了另一个 Agent 的帖子被判为重放 → 新提及丢失，被 @ 的 Agent
// 永远收不到通知。`content_hash` 覆盖的是正文，不覆盖显式 mentions。
//
// 期望：显式 mentions 不同时应视为不同请求（或合并进旧帖），不能静默丢弃。
// ===========================================================================
#[test]
fn reply_dedup_does_not_silently_drop_new_explicit_mentions() {
    let (repo, _root) = open("pipi-forum-v2-mention-dedup");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();

    let mut first = post_draft("b", "请看下", None);
    first.mentions = vec!["alice".into()];
    let a = repo.append_post(&thread.id, &first).unwrap();

    let mut second = post_draft("b", "请看下", None);
    second.mentions = vec!["bob".into()];
    let b = repo.append_post(&thread.id, &second).unwrap();

    assert_ne!(
        a.id, b.id,
        "显式 @bob 的帖子被判为 @alice 帖的重放，bob 的提及被静默丢弃"
    );
}

// ===========================================================================
// F6（低）: 去重命中时调用方拿到「看起来刚创建」的旧帖，且新 origin 被丢弃
//
// `append_post` 幂等命中直接返回已存在的 `Post`（旧 id/seq/created/origin），
// 没有「这是已有帖」的信号；`ForumReplyTool` 仍打印「已回帖 #seq」。调用方
// 无法区分「新建」与「重放」，审计用的新 origin（如 `run:2`）也随之丢失。
//
// 期望：要么保留本次 origin（可另存重放记录），要么让返回值/工具输出显式表明
// 命中了已有帖（例如 `deduplicated: true`）。
// ===========================================================================
#[test]
fn dedup_hit_should_not_silently_discard_the_retry_origin() {
    let (repo, _root) = open("pipi-forum-v2-dedup-origin");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();

    let mut first = post_draft("b", "同一条", None);
    first.origin = Some("run:1".into());
    repo.append_post(&thread.id, &first).unwrap();

    let mut retry = post_draft("b", "同一条", None);
    retry.origin = Some("run:2".into());
    let returned = repo.append_post(&thread.id, &retry).unwrap();

    assert_eq!(
        returned.origin.as_deref(),
        Some("run:2"),
        "重放命中去重后，本次 origin（run:2）被静默丢弃，返回值不可区分新建/重放"
    );
}

// ===========================================================================
// F7（低）: 首帖永远没有 `origin`（`NewThread` 无该字段、`create_thread` 写 None）
//
// 设计文档 §4 说 `origin` 是「工具路径的 `run:<runId>`」审计 token。但 origin
// 只存在于 `NewPost`：`forum_post`（建主题）根本无从传入，`create_thread`
// 也硬编码 `origin: None`。于是每个主题的首帖都缺溯源标记，工具路径写的第一条
// 内容无法审计；Phase 2 的 `(sourcePost, agent, runId)` 幂等键也拿不到它。
//
// 期望：首帖也带 origin（NewThread 需提供入口）。
// ===========================================================================
#[test]
fn thread_first_post_should_carry_origin() {
    let (repo, _root) = open("pipi-forum-v2-origin-firstpost");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();
    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    assert!(
        posts[0].origin.is_some(),
        "主题首帖的 origin 恒为 None，工具路径的溯源 token 无法落到首帖"
    );
}

// ===========================================================================
// 正向自证伪 1: 新字段正常往返 + 未知字段被忽略（设计文档 §4 的字段级断言成立）
// ===========================================================================
#[test]
fn new_fields_roundtrip_and_unknown_fields_are_ignored() {
    let (repo, _root) = open("pipi-forum-v2-roundtrip");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();
    let mut draft = post_draft("b", "正文", None);
    draft.origin = Some("run:42".into());
    let post = repo.append_post(&thread.id, &draft).unwrap();

    assert!(post.content_hash.as_deref().unwrap().starts_with("sha256:"));
    let read = repo.posts(&thread.id, PostRange::default()).unwrap();
    let read = read.iter().find(|p| p.id == post.id).unwrap();
    assert_eq!(read.content_hash, post.content_hash, "contentHash 往返丢失");
    assert_eq!(read.origin.as_deref(), Some("run:42"), "origin 往返丢失");
}

// ===========================================================================
// 正向自证伪 2: 新读者读旧文件（缺全部新字段）正常，帖子不被丢弃。
// ===========================================================================
#[test]
fn old_post_without_new_fields_is_readable() {
    let (repo, root) = open("pipi-forum-v2-oldpost");
    let thread = repo
        .create_thread(&thread_draft("host", "t", "起帖"))
        .unwrap();
    let dir = posts_dir(&root, &thread.id);
    fs::write(
        dir.join("0002-old.md"),
        "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\nvia = \"tool\"\n+++\n\n旧格式正文\n",
    )
    .unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let post = posts.iter().find(|p| p.seq == 2).expect("旧格式帖子应可读");
    assert_eq!(
        post.content_hash, None,
        "缺 content_hash 应为 None 而非报错"
    );
    assert_eq!(post.origin, None);
}

// 让 ThreadFilter / PostVia 的 import 总被用到。
#[test]
fn probe_imports_are_used() {
    let (repo, _root) = open("pipi-forum-v2-probe");
    let thread = repo.create_thread(&thread_draft("host", "x", "y")).unwrap();
    let _ = repo.list_threads(&ThreadFilter::default(), 10).unwrap();
    assert!(thread.id.contains('-'));
}
