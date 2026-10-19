//! 对抗式审查（格式 / 数据完整性 / 演进兼容）的最小复现。
//!
//! 本文件**只做演示**，不修改任何生产代码。用例断言的是「应当成立的性质」：
//! 某个测试失败，说明它复现了一个真实缺陷（或一处文档与实现不一致）。
//! 报告见 `docs/research/forum/review-data.md`。
//!
//! 运行：`cargo test -p pipi-forum --test adversarial_data`
//!
//! 注：`format` 模块（`encode_post` / `decode_post`）在 crate 内私有，集成测试
//! 只能走公开入口，因此这里全部通过 `FileForumRepo` 读写**手工构造的磁盘文件**
//! 来接触编解码边界 —— 这正是「文件迟早被手改坏」的对抗场景。

use std::fs;
use std::path::{Path, PathBuf};

use pipi_forum::FileForumRepo;
use pipi_protocol::forum::{ForumRepo, NewThread, PostRange, PostVia, SearchOptions, ThreadFilter};

fn open(prefix: &str) -> (FileForumRepo, PathBuf) {
    let root = pipi_test_support::temp_dir(prefix);
    let repo = FileForumRepo::open(root.clone()).unwrap();
    (repo, root)
}

fn thread_draft(body: &str) -> NewThread {
    NewThread {
        title: "t".into(),
        author: "host".into(),
        body: body.into(),
        tags: vec![],
        mentions: vec![],
        via: PostVia::Tool,
        origin: Some("test-run".into()),
    }
}

fn posts_dir(root: &Path, id: &str) -> PathBuf {
    let dir = root.join("threads").join(id).join("posts");
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 手工拼一条标准帖子文件（frontmatter 字段名与生产代码一致：camelCase）。
fn raw_post(id: &str, seq: u64, author: &str, created: u64, via: &str, body: &str) -> String {
    format!(
        "+++\nid = \"{id}\"\nseq = {seq}\nauthor = \"{author}\"\ncreated = {created}\nvia = \"{via}\"\n+++\n\n{body}\n"
    )
}

// ===========================================================================
// 1. 编码不保真：一条非 UTF-8 帖子文件会让**整个主题**读不出来
//
// `read_posts_in` 对 `decode_post` 的失败是「跳过坏文件」，但对
// `fs::read_to_string` 的失败用 `?` 直接向上抛 —— 一个二进制/非 UTF-8 的
// `.md` 文件（比如用户误存、工具写了一半）会让 `posts()` / `list_threads()`
// 整体报错，同一目录里完好的帖子也看不到。同目录的目录条目（`evil.md/`）同理。
//
// 期望：坏文件应像坏 frontmatter 一样被跳过，其余帖子照常可读。
// ===========================================================================
#[test]
fn non_utf8_post_does_not_kill_the_whole_thread() {
    let (repo, root) = open("pipi-forum-adv-nonutf8");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);

    fs::write(
        dir.join("0002-good.md"),
        raw_post("a-2", 2, "b", 20, "tool", "好帖"),
    )
    .unwrap();
    // 非法 UTF-8 字节：模拟二进制/截断写入。
    fs::write(dir.join("0003-bad.md"), [0xff, 0xfe, 0x00, 0x80]).unwrap();

    let posts = repo
        .posts(&thread.id, PostRange::default())
        .expect("坏文件不应让整个主题不可读");
    assert!(
        posts.iter().any(|post| post.seq == 2),
        "同目录里完好的帖子应当仍可读，实际得到 {posts:?}"
    );
    // 列表也应当照常看到该主题，而不是因为它读不出来就静默消失。
    let listed = repo.list_threads(&ThreadFilter::default(), 10).unwrap();
    assert!(
        listed.iter().any(|s| s.thread.id == thread.id),
        "含坏文件的主题不应从列表消失"
    );
}

// ===========================================================================
// 2. UTF-8 BOM：编辑器带 BOM 保存的帖子被静默丢弃，post_count 与真值不符
//
// `decode_post` 只 `strip_prefix("+++")`，不剥 BOM。带 BOM 的文件第一条
// 前缀是 `\u{feff}+++`，直接判定「缺少起始围栏」→ 跳过。没有告警，`post_count`
// 静默少计。
// ===========================================================================
#[test]
fn bom_prefixed_post_is_not_silently_dropped() {
    let (repo, root) = open("pipi-forum-adv-bom");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);

    let mut bommed = String::from('\u{feff}');
    bommed.push_str(&raw_post("a-2", 2, "b", 20, "tool", "带 BOM 的正文"));
    fs::write(dir.join("0002-bom.md"), bommed).unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    assert!(
        posts.iter().any(|post| post.seq == 2),
        "带 BOM 的帖子不应被静默丢弃，实际只有 {posts:?}"
    );
}

// ===========================================================================
// 3. 正文不保真：`\r\n` / 单独 `\r` 被全局归一成 `\n`，`trim_matches('\n')`
//    又剥掉正文首尾的空行
//
// 解码第一行 `content.replace("\r\n", "\n").replace('\r', "\n")` 作用于**整份
// 文件**（含正文）：正文里刻意保留的 CRLF 或孤立 CR（少见但真实，例如从
// Windows 粘贴、旧 Mac）被改写。随后 `body.trim_matches('\n')` 去掉首尾所有
// 换行，正文末尾的空行（代码块后常见）丢失。
//
// 期望：手写进文件的正文应原样读回（`\n` 归一可接受，但换行数应保持）。
// ===========================================================================
#[test]
fn body_newlines_roundtrip_through_disk() {
    let (repo, root) = open("pipi-forum-adv-body");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);

    // 正文：首部空行 + 独立 CR + 尾部空行。用 `\r` 模拟「孤立 CR」。
    let body = "\n\n第一行\r\n\r\n尾部\n\n";
    let content = format!(
        "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\nvia = \"tool\"\n+++\n\n{body}\n"
    );
    fs::write(dir.join("0002-body.md"), content).unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let read = posts.iter().find(|p| p.seq == 2).expect("帖子应可读");
    // 首尾空行：`trim_matches('\n')` 会剥掉；真实值应保留。
    assert!(
        read.body.starts_with('\n'),
        "正文首部空行被丢弃：{:?}",
        read.body
    );
    // 孤立 CR：被归一成 \n，不再是原字节。
    assert!(
        read.body.contains('\r'),
        "正文里的孤立 CR 被改写成 \\n：{:?}",
        read.body
    );
}

// ===========================================================================
// 4. 手改 frontmatter 的 camelCase 陷阱：写 `reply_to` 会被静默忽略
//
// 设计文档 §4 的 DTO 用蛇形名 `reply_to`，但 `PostFrontmatter` 标了
// `#[serde(rename_all = "camelCase")]`，磁盘键是 `replyTo`。照文档手写
// `reply_to = "..."` 时，toml 把未知键**静默忽略**，`replyTo` 变成 None，
// 回帖指向关系无声丢失 —— 这正是「未知字段照读」换来的代价。
//
// 期望：文档承诺的字段名（或用例中的两种拼写）至少有一种能得到映射，
// 或者未知键应当报错而非静默丢弃。
// ===========================================================================
#[test]
fn snake_case_reply_to_in_hand_edited_frontmatter_is_honored() {
    let (repo, root) = open("pipi-forum-adv-replyto");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);

    let content = "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\nvia = \"tool\"\nreply_to = \"a-1\"\n+++\n\n回复\n";
    fs::write(dir.join("0002-reply.md"), content).unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let reply = posts.iter().find(|p| p.seq == 2).expect("帖子应可读");
    assert_eq!(
        reply.reply_to.as_deref(),
        Some("a-1"),
        "手写 `reply_to` 被静默忽略（磁盘键实际是 `replyTo`）"
    );
}

// ===========================================================================
// 5. schema 演进（向后）：`set_status` / `set_pinned` 回写 thread.toml 时
//    丢弃未知字段
//
// 设计文档 §4 称 `origin` / `board` / `updated_at` 等可 `#[serde(default)]`
// 增量加入、旧文件照读。读没问题，但 **thread.toml 会被 read-modify-write**：
// `set_status` / `set_pinned` 用 `encode_thread(Thread)` 覆盖写，未知字段（例如
// 未来版本写入的 `board`）在旧二进制里被解析忽略、回写时直接消失 —— 数据静默
// 丢失。帖子文件从不回写，所以只有 thread.toml 中招。
//
// 期望：老读者回写不应吃掉它不认识的新字段。
// ===========================================================================
#[test]
fn rewrite_does_not_drop_unknown_thread_fields() {
    let (repo, root) = open("pipi-forum-adv-thread-clobber");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let path = root.join("threads").join(&thread.id).join("thread.toml");
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str("board = \"design\"\n");
    fs::write(&path, &content).unwrap();

    repo.set_pinned(&thread.id, true).unwrap();

    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("board"),
        "回写 thread.toml 吃掉了未知字段 `board`，回写后内容：\n{after}"
    );
}

// ===========================================================================
// 6. schema 演进（向前）：新增枚举变体会让**旧读者整条读不出**
//
// `#[serde(default)]` 只覆盖「字段缺失」，覆盖不了「枚举出现未知变体」。
// 设计文档 §6 已规划 `settled` / `locked` 状态；一旦新版本写入
// `status = "settled"`，旧二进制 `toml::from_str::<Thread>` 直接报错 →
// `thread()` 失败、`list_threads` 跳过该主题 —— 主题在旧版界面里「凭空消失」。
//
// 期望：未知状态不应让整条主题不可读（至少降级为可读）。
// ===========================================================================
#[test]
fn future_thread_status_does_not_hide_the_thread() {
    let (repo, root) = open("pipi-forum-adv-status-enum");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let path = root.join("threads").join(&thread.id).join("thread.toml");
    let content = fs::read_to_string(&path).unwrap();
    // 模拟未来版本把 status 升级为枚举新值。
    let future = content.replace("status = \"open\"", "status = \"settled\"");
    assert_ne!(future, content, "未找到 status 字段，fixture 需更新");
    fs::write(&path, future).unwrap();

    assert!(
        repo.thread(&thread.id).is_ok(),
        "旧读者遇到未来枚举值 `settled` 直接失败，主题消失"
    );
}

// ===========================================================================
// 7. schema 演进（向前）：帖子的 `via` 出现未知变体时整条帖被静默跳过
//    （与 §6 同源，落在 PostVia 上）
// ===========================================================================
#[test]
fn future_post_via_variant_does_not_hide_the_post() {
    let (repo, root) = open("pipi-forum-adv-via-enum");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);
    fs::write(
        dir.join("0002-system.md"),
        raw_post("a-2", 2, "system", 20, "system", "系统帖"),
    )
    .unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    assert!(
        posts.iter().any(|p| p.seq == 2),
        "未来 via 变体 `system` 让帖子被静默跳过：{posts:?}"
    );
}

// ===========================================================================
// 8. `PostFrontmatter::via` 缺 `#[serde(default)]`（DTO 有、磁盘结构没有）
//
// `pipi_protocol::forum::Post` 的 `via` 标了 `#[serde(default)]`，但磁盘
// `PostFrontmatter.via` 没有。结果：一份未写 `via` 的最小 frontmatter（手写、
// 早期格式、其他工具生成）会被判定解析失败并跳过 —— 契约层与磁盘层不对称。
//
// 期望：缺 `via` 时按默认 `Tool` 解码。
// ===========================================================================
#[test]
fn missing_via_defaults_instead_of_dropping_post() {
    let (repo, root) = open("pipi-forum-adv-missing-via");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);
    fs::write(
        dir.join("0002-novia.md"),
        "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\n+++\n\n无 via 的正文\n",
    )
    .unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let post = posts.iter().find(|p| p.seq == 2);
    assert!(post.is_some(), "缺 via 的帖子被静默跳过：{posts:?}");
}

// ===========================================================================
// 9. frontmatter 边界：结束围栏用 `find("\n+++")` 定位，多行 TOML 字符串里
//    出现 `+++` 行会被提前截断
//
// `decode_post` 找的是**第一个** `\n+++`，不校验它是否独占一行。TOML 的多行
// 基本字符串（`"""..."""`）允许值内出现 `+++` 行；这样一条手工 frontmatter
// 会被拦腰截断 → frontmatter 解析失败 → 帖子整条被跳过。
//
// 期望：围栏必须独占一行，值内的 `+++` 不应截断 frontmatter。
// ===========================================================================
#[test]
fn fence_inside_multiline_toml_value_does_not_truncate() {
    let (repo, root) = open("pipi-forum-adv-fence");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);
    let content = "+++\nid = \"a-2\"\nseq = 2\nauthor = \"\"\"first\n+++\nsecond\"\"\"\ncreated = 20\nvia = \"tool\"\n+++\n\n正文\n";
    fs::write(dir.join("0002-fence.md"), content).unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let post = posts
        .iter()
        .find(|p| p.seq == 2)
        .unwrap_or_else(|| panic!("多行 TOML 值含 `+++` 时整条帖被跳过：{posts:?}"));
    assert!(
        post.author.contains("second"),
        "多行 author 被提前截断：{:?}",
        post.author
    );
}

// ===========================================================================
// 10. 派生字段：`last_active` 取「按 seq 最后一条」的 created，而非最大 created
//
// 手改 / 时钟回拨 / 导入都会让某条帖子的 `created` 小于前一条。此时
// `last_active` 会「倒退」，把主题排到更旧的位置；列表排序（按 last_active）
// 因此与真实最新活动不符。派生值本身没错，但口径应当稳定为 max(created)。
// ===========================================================================
#[test]
fn last_active_is_max_created_not_last_by_seq() {
    let (repo, root) = open("pipi-forum-adv-last-active");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);
    // seq 越大 created 反而越小（时钟回拨 / 手改）。
    fs::write(
        dir.join("0002-skew.md"),
        raw_post("a-2", 2, "b", 50, "tool", "晚发早戳"),
    )
    .unwrap();

    let list = repo.list_threads(&ThreadFilter::default(), 10).unwrap();
    let summary = list.iter().find(|s| s.thread.id == thread.id).unwrap();
    let max_created = repo
        .posts(&thread.id, PostRange::default())
        .unwrap()
        .iter()
        .map(|p| p.created)
        .max()
        .unwrap();
    assert_eq!(
        summary.last_active, max_created,
        "last_active 取了 seq 最大者的 created（{}），而非真实最大 created（{}）",
        summary.last_active, max_created
    );
}

// ===========================================================================
// 11. 文件名排序 = 序号排序：约定已改为 8 位零填充（原为 4 位，seq ≥ 10000 时
//     字典序反转）。此处按**新约定**命名两个帖子，断言行名字典序仍等于序号序。
//     边界：`seq < 10^8`（读取路径按 frontmatter seq 数值排序，文件名序号仅作人读提示）。
// ===========================================================================
#[test]
fn filename_sort_equals_seq_sort_with_eight_digit_padding() {
    let (repo, root) = open("pipi-forum-adv-seq-width");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);
    fs::write(
        dir.join("00009999-aaaa.md"),
        raw_post("a-9999", 9999, "b", 1, "tool", "p9999"),
    )
    .unwrap();
    fs::write(
        dir.join("00010000-bbbb.md"),
        raw_post("a-10000", 10000, "b", 2, "tool", "p10000"),
    )
    .unwrap();

    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        // 只保留本用例写入的两条（主题还带一条自动创建的首帖）。
        .filter(|n| n.ends_with("aaaa.md") || n.ends_with("bbbb.md"))
        .collect();
    names.sort();
    let by_name: Vec<u32> = names
        .iter()
        .map(|n| n.split('-').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        by_name,
        vec![9999, 10000],
        "8 位零填充下文件名排序应等于序号排序：{names:?}"
    );
}

// ===========================================================================
// 12. 正向自证伪：未知**字段**确实被忽略（设计文档 §4 的断言在字段维度成立）
//     —— 与用例 5/6/7 对照，界定缺陷边界「字段可以、枚举不行、回写会丢」。
// ===========================================================================
#[test]
fn unknown_post_field_is_ignored_and_body_survives() {
    let (repo, root) = open("pipi-forum-adv-forward-compat");
    let thread = repo.create_thread(&thread_draft("起帖")).unwrap();
    let dir = posts_dir(&root, &thread.id);
    let content = "+++\nid = \"a-2\"\nseq = 2\nauthor = \"b\"\ncreated = 20\nvia = \"tool\"\norigin = \"run-1\"\n+++\n\n正文\n";
    fs::write(dir.join("0002-future.md"), content).unwrap();

    let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
    let post = posts
        .iter()
        .find(|p| p.seq == 2)
        .expect("未知字段 origin 不应让帖子不可读");
    assert_eq!(post.author, "b");
}

// ===========================================================================
// 13. 正向自证伪：新读者读旧文件（缺可选字段）正常
// ===========================================================================
#[test]
fn old_minimal_thread_file_is_readable() {
    let (repo, root) = open("pipi-forum-adv-old-file");
    let id = "1758000000000-ab12cd34";
    let dir = root.join("threads").join(id);
    fs::create_dir_all(dir.join("posts")).unwrap();
    // 只有必填字段：tags/status/pinned 全缺。
    fs::write(
        dir.join("thread.toml"),
        format!("id = \"{id}\"\ntitle = \"旧格式\"\nauthor = \"a\"\ncreated = 1\n"),
    )
    .unwrap();
    let thread = repo.thread(id).expect("缺可选字段的旧文件应当照读");
    assert!(thread.tags.is_empty());
    assert!(!thread.pinned);
}

// ===========================================================================
// 14. 检索语义 / 片段截断：`to_lowercase` 改变字节长度时，`&body[..at]` 越界
//
// `match_snippet`（`file.rs`）在 `body.to_lowercase()` 得到的 haystack 上取**字节
// 偏移** `at`，再切原始 `body`：`body[..at]`。`char::to_lowercase` 会改变 UTF-8
// 字节长度，缩短时 `at` 落进原 body 的多字节字符中间 → `byte index is not a char
// boundary` panic。U+1E9E LATIN CAPITAL LETTER SHARP S（3 字节）小写为 ß（2 字节）
// 即触发：正文 `ẞabc`、检索 `abc`。（与 `adversarial_code.rs` 的发现 A 同源，
// 该文件用 U+212A 触发；此处覆盖本报告的 Unicode/截断条目）
//
// 同一原因也会让土耳其语 İ（U+0130，2 字节）小写为 i + U+0307（3 字节）等场景的
// 片段窗口错位。片段本应按 `char` 边界截断（代码后半段的 `body.chars()` 已是字符
// 口径，唯独 `body[..at]` 是字节口径）。
// ===========================================================================
#[test]
fn search_snippet_keeps_char_boundaries_on_unicode_case_fold() {
    let (repo, _root) = open("pipi-forum-adv-unicode-search");
    // 正文以「小写后缩短」的字符开头，命中词在其后。
    repo.create_thread(&thread_draft("ẞabc")).unwrap();

    let hits = repo
        .search("abc", SearchOptions::default())
        .expect("search 不应对合法 Unicode 正文 panic");
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].snippet.contains("abc"),
        "片段应包含命中词：{:?}",
        hits[0].snippet
    );
}
