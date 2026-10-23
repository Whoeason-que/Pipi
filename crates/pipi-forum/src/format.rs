//! 磁盘格式：论坛 id、主题/帖子的编解码。
//!
//! 一切皆文件。一个版面根目录长这样：
//!
//! ```text
//! <root>/
//! ├── threads/
//! │   └── <thread-id>/
//! │       ├── thread.toml              # 主题元数据
//! │       └── posts/
//! │           ├── 00000001-<post-id>.md   # 一条帖子一个文件（frontmatter + 正文）
//! │           └── 00000002-<post-id>.md
//! └── .archive/
//!     └── <thread-id>/                 # 归档主题（结构同上）
//! ```
//!
//! 帖子用 **TOML frontmatter**（`+++` 围栏）承载结构化字段、Markdown 承载正文。
//! 选 TOML 而非 YAML 是刻意的：`toml` 已是工作区依赖（零新增）、可精确保序回写、
//! 原生支持数组，且不引入 YAML 供应链。
//!
//! **正文逐字节保真**：`+++` 围栏之后的内容原样保存（不归一化换行、不 trim）。
//! 编码在结束围栏行之后只隔一个换行（围栏行的行尾）再看正文；解码剥掉该换行，
//! 因此我们写出的文件往返无损，手抄文件也容错（不额外剥空行）。
//!
//! **向后兼容**：未知字段被忽略（帖子文件从不回写，故不会丢；`thread.toml` 的回写
//! 走 TOML 表合并，见 `file.rs`），未知枚举值降级为兜底变体（`#[serde(other)]`）。
//!
//! 格式被 [`pipi_protocol::forum::ForumRepo`] 端口隔离 —— 将来若要换 YAML
//! frontmatter 以贴合 Obsidian properties，只换载体实现，不动调用点。

use pipi_protocol::forum::{Post, PostVia, Thread};
use serde::{Deserialize, Serialize};

/// frontmatter 围栏。
pub const FRONTMATTER_FENCE: &str = "+++";

/// 生成一个论坛 id（主题 id / 帖子 id 共用）：`{millis}-{8hex}`。
///
/// 时间前缀便于人工排序，随机尾缀避免同毫秒碰撞。会话 id 用的是完整 32 位
/// uuid 尾缀；论坛取前 8 位（本地单机尺度足够，路径更短更好读）。
/// [`pipi_protocol::forum::validate_forum_id`] 保证路径安全。
pub fn new_forum_id() -> String {
    let millis = pipi_protocol::now_millis();
    let random = pipi_protocol::new_id();
    format!("{millis}-{}", &random[..8])
}

/// 帖子文件名：`<seq 八位零填充>-<post-id>.md`。
///
/// 零填充到 8 位，使「按字典序排文件名」在 `seq < 10^8` 内等于「按序号排序」。
/// 读取路径按 frontmatter 的 `seq` 数值排序，文件名序号只作人读提示。
pub fn post_filename(post: &Post) -> String {
    format!("{:08}-{}.md", post.seq, post.id)
}

/// 帖子 frontmatter 字段（与 [`Post`] 的结构化部分一一对应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostFrontmatter {
    id: String,
    seq: u64,
    author: String,
    created: u64,
    /// 缺失时按默认（`Tool`）解码，避免手写/旧格式帖子被判为坏文件丢弃。
    #[serde(default)]
    via: PostVia,
    /// 磁盘键是 `replyTo`；同时接受手写常用的 `reply_to`。
    #[serde(default, alias = "reply_to", skip_serializing_if = "Option::is_none")]
    reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_id: Option<String>,
    #[serde(default)]
    causality_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}

/// 把一条帖子编码成文件内容（`+++` frontmatter + 逐字节保真的正文）。
pub fn encode_post(post: &Post) -> String {
    let frontmatter = PostFrontmatter {
        id: post.id.clone(),
        seq: post.seq,
        author: post.author.clone(),
        created: post.created,
        via: post.via,
        reply_to: post.reply_to.clone(),
        mentions: post.mentions.clone(),
        root_id: post.root_id.clone(),
        causality_depth: post.causality_depth,
        content_hash: post.content_hash.clone(),
        origin: post.origin.clone(),
    };
    // 结构体字段固定、无非法 TOML 值，序列化不会失败。
    let toml = toml::to_string(&frontmatter).expect("post frontmatter is always serializable");
    format!(
        "{FRONTMATTER_FENCE}\n{toml}{FRONTMATTER_FENCE}\n\n{}",
        post.body
    )
}

/// 从文件内容解析一条帖子。
///
/// **宽松解析**：frontmatter 先当 `toml::Table` 读，再逐字段取值。必需字段
/// （`id`/`seq`/`author`/`created`）缺失才判为坏帖；**可选字段类型不符时只
/// 丢这个字段**（降级为 `None`/默认），不把整条帖子判为坏文件。键名同时接受
/// camelCase 与 snake_case。
pub fn decode_post(content: &str) -> Result<Post, String> {
    let (table, body) = split_frontmatter(content)?;
    let id = string_field(&table, "id").ok_or("帖子缺少 id")?;
    let seq = integer_field(&table, "seq").ok_or("帖子缺少 seq")? as u64;
    let author = string_field(&table, "author").ok_or("帖子缺少 author")?;
    let created = integer_field(&table, "created").ok_or("帖子缺少 created")? as u64;
    Ok(Post {
        id,
        seq,
        author,
        created,
        via: string_field(&table, "via")
            .map(|value| match value.as_str() {
                "tool" => PostVia::Tool,
                "cli" => PostVia::Cli,
                "web" => PostVia::Web,
                _ => PostVia::Other,
            })
            .unwrap_or_default(),
        reply_to: string_field_alias(&table, &["replyTo", "reply_to"]),
        mentions: string_list_field(&table, "mentions"),
        root_id: string_field_alias(&table, &["rootId", "root_id"]),
        causality_depth: integer_field_alias(&table, &["causalityDepth", "causality_depth"])
            .filter(|value| *value >= 0)
            .unwrap_or(0) as u32,
        content_hash: string_field_alias(&table, &["contentHash", "content_hash"]),
        origin: string_field(&table, "origin"),
        body,
    })
}

fn string_field(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

fn string_field_alias(table: &toml::Table, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string_field(table, key))
}

fn integer_field(table: &toml::Table, key: &str) -> Option<i64> {
    table.get(key).and_then(toml::Value::as_integer)
}

fn integer_field_alias(table: &toml::Table, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| integer_field(table, key))
}

fn string_list_field(table: &toml::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 切出 frontmatter 表与正文。
///
/// 结束围栏是**独占一行**的 `+++`；若某个候选围栏切出的 frontmatter 不是合法
/// TOML（例如多行 TOML 字符串里正好有一行 `+++`），就继续找下一个候选。带 BOM
/// 的文件先剥 BOM。正文逐字节保留。
fn split_frontmatter(content: &str) -> Result<(toml::Table, String), String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = content.split_inclusive('\n');
    let first = lines.next().ok_or_else(|| "Post is empty".to_string())?;
    if trim_line_end(first) != FRONTMATTER_FENCE {
        return Err("Post is missing frontmatter opening fence `+++`".into());
    }
    let frontmatter_start = first.len();
    let mut cursor = frontmatter_start;
    let mut last_error: Option<String> = None;
    for line in lines {
        let line_start = cursor;
        cursor += line.len();
        if trim_line_end(line) != FRONTMATTER_FENCE {
            continue;
        }
        let candidate = &content[frontmatter_start..line_start];
        match toml::from_str::<toml::Table>(candidate) {
            Ok(table) => {
                let after = &content[cursor..];
                return Ok((table, strip_separator(after).to_string()));
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    match last_error {
        Some(error) => Err(format!("Failed to parse post frontmatter: {error}")),
        None => Err("Post is missing frontmatter closing fence `+++`".into()),
    }
}

fn trim_line_end(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// 剥掉结尾围栏行之后的那一个换行（我们只写一个；手抄文件不额外剥空行）。
fn strip_separator(after: &str) -> &str {
    if let Some(rest) = after.strip_prefix("\r\n") {
        return rest;
    }
    if let Some(rest) = after.strip_prefix('\n') {
        return rest;
    }
    after
}

/// 把主题元数据编码成 `thread.toml`（仅用于**新建**；更新走表合并，见 `file.rs`）。
pub fn encode_thread(thread: &Thread) -> String {
    toml::to_string(thread).expect("thread metadata is always serializable")
}

/// 解析 `thread.toml`。未知枚举值降级为兜底变体，不让整条主题读不出。
pub fn decode_thread(content: &str) -> Result<Thread, String> {
    toml::from_str(content).map_err(|error| format!("主题元数据解析失败: {error}"))
}

/// 大小写不敏感查找，返回**原始串**里的字节下标（落在字符边界上）。
///
/// 不能直接拿 `haystack.to_lowercase().find(...)` 的字节偏移去切 `haystack`：
/// `char::to_lowercase` 会改变 UTF-8 字节长度（如 `\u{212A}` 3 字节 → `k` 1 字节），
/// 使折叠后的偏移落在原串的多字节字符中间，造成 panic 或片段错位。这里逐字符折叠
/// 并记录「折叠串偏移 → 原串字节下标」的映射，把命中位置映射回原串的字符边界。
pub fn find_case_insensitive(haystack: &str, needle_lowercase: &str) -> Option<usize> {
    if needle_lowercase.is_empty() {
        return None;
    }
    let mut lowered = String::new();
    // (折叠串中的字节偏移, 原串中的字节下标)，按原串字符递增。
    let mut origin: Vec<(usize, usize)> = Vec::new();
    for (byte_index, character) in haystack.char_indices() {
        origin.push((lowered.len(), byte_index));
        for folded in character.to_lowercase() {
            lowered.push(folded);
        }
    }
    origin.push((lowered.len(), haystack.len()));
    let found = lowered.find(needle_lowercase)?;
    let index = origin
        .partition_point(|(lowered_offset, _)| *lowered_offset <= found)
        .saturating_sub(1);
    Some(origin[index].1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::forum::{PostVia, ThreadStatus};

    fn sample_post() -> Post {
        Post {
            id: "1758000010000-cd34beef".into(),
            seq: 2,
            author: "reviewer".into(),
            created: 1758000010000,
            via: PostVia::Tool,
            reply_to: Some("1758000000000-ab12abcd".into()),
            mentions: vec!["builder".into(), "planner".into()],
            root_id: Some("1758000000000-ab12abcd".into()),
            causality_depth: 1,
            content_hash: Some("sha256:deadbeef".into()),
            origin: Some("run:1".into()),
            body: "# 标题\n\n正文含 `+++` 与 <xml> 也不受影响。\n\n- 一\n- 二".into(),
        }
    }

    #[test]
    fn post_roundtrips_through_frontmatter() {
        let post = sample_post();
        let encoded = encode_post(&post);
        assert!(encoded.starts_with("+++\n"));
        assert_eq!(decode_post(&encoded).unwrap(), post);
    }

    #[test]
    fn body_is_byte_preserved_including_edge_newlines() {
        for body in [
            "",
            "\n",
            "\n\n前导空行",
            "尾部空行\n\n",
            "CRLF\r\n保留\r\n",
            "孤立\r回车",
            "第一行\n+++\n像围栏\n+++\n结尾",
        ] {
            let mut post = sample_post();
            post.body = body.into();
            assert_eq!(
                decode_post(&encode_post(&post)).unwrap().body,
                body,
                "正文未逐字节保真: {body:?}"
            );
        }
    }

    #[test]
    fn missing_or_broken_frontmatter_is_rejected() {
        assert!(decode_post("正文没有 frontmatter").is_err());
        assert!(decode_post("+++\nseq = 1\n没有结束围栏").is_err());
        assert!(decode_post("+++\nseq = \"not a number\"\n+++\n正文").is_err());
    }

    #[test]
    fn bom_and_snake_case_and_missing_via_are_tolerated() {
        let content = "\u{feff}+++\nid = \"a\"\nseq = 1\nauthor = \"b\"\ncreated = 1\nreply_to = \"a-0\"\n+++\n\n正文";
        let post = decode_post(content).unwrap();
        assert_eq!(post.via, PostVia::Other, "缺 via 应默认 Other（不是 Tool）");
        assert_eq!(post.reply_to.as_deref(), Some("a-0"), "reply_to 别名应生效");
        assert_eq!(post.body, "正文");
    }

    #[test]
    fn fence_inside_multiline_toml_value_does_not_truncate() {
        let content = "+++\nid = \"a\"\nseq = 2\nauthor = \"\"\"first\n+++\nsecond\"\"\"\ncreated = 2\nvia = \"tool\"\n+++\n\n正文";
        let post = decode_post(content).unwrap();
        assert!(
            post.author.contains("second"),
            "被提前截断: {:?}",
            post.author
        );
        assert_eq!(post.body, "正文");
    }

    #[test]
    fn unknown_enum_values_do_not_fail_the_record() {
        let content =
            "+++\nid = \"a\"\nseq = 1\nauthor = \"b\"\ncreated = 1\nvia = \"system\"\n+++\n正文";
        assert_eq!(decode_post(content).unwrap().via, PostVia::Other);
        let thread = Thread {
            id: "1758000000000-ab12cd34".into(),
            title: "并发模型怎么定".into(),
            author: "builder".into(),
            created: 1758000000000,
            tags: vec!["design".into()],
            status: ThreadStatus::Open,
            pinned: false,
        };
        assert_eq!(decode_thread(&encode_thread(&thread)).unwrap(), thread);
        let raw = "id = \"x\"\ntitle = \"t\"\nauthor = \"a\"\ncreated = 1\nstatus = \"settled\"\n";
        assert_eq!(decode_thread(raw).unwrap().status, ThreadStatus::Unknown);
    }

    #[test]
    fn post_filename_sorts_by_seq() {
        let mut a = sample_post();
        a.seq = 10_000;
        let mut b = sample_post();
        b.seq = 9999;
        let mut names = [post_filename(&a), post_filename(&b)];
        names.sort();
        assert!(names[0].starts_with("00009999-"), "{names:?}");
        assert!(names[1].starts_with("00010000-"), "{names:?}");
    }

    #[test]
    fn case_insensitive_find_maps_back_to_char_boundaries() {
        // U+212A KELVIN SIGN 折叠成 'k'（3 字节 → 1 字节）。
        let haystack = "\u{212A}abc";
        let at = find_case_insensitive(haystack, "abc").unwrap();
        assert!(haystack.is_char_boundary(at));
        assert_eq!(&haystack[at..], "abc");
        // U+0130 折叠成两个字符，偏移增长。
        let haystack = "\u{0130}x";
        let at = find_case_insensitive(haystack, "x").unwrap();
        assert!(haystack.is_char_boundary(at));
        assert_eq!(&haystack[at..], "x");
        assert_eq!(find_case_insensitive("abc", "zzz"), None);
    }

    #[test]
    fn generated_ids_are_valid_and_unique() {
        let a = new_forum_id();
        let b = new_forum_id();
        assert_ne!(a, b);
        assert!(pipi_protocol::forum::validate_forum_id(&a).is_ok());
        assert!(pipi_protocol::forum::validate_forum_id(&b).is_ok());
    }
}
