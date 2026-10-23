//! 本地论坛的**格式**与**存储端口**。
//!
//! 「Agent 本地论坛」是一个纯文件的讨论区：Agent 与人围绕**主题帖**（thread）
//! 异步发帖/回帖，支持 `@提及`、标签、搜索与归档。它相对 `run_agent` 的价值是
//! **异步 + 多对多 + 讨论可沉淀**。
//!
//! 与会话一样，**格式、语义与载体分离**：
//!
//! - 本模块定义数据形状（[`Thread`] / [`Post`]）、写入草稿（[`NewThread`] /
//!   [`NewPost`]）、以及查询词汇（[`ThreadFilter`] / [`PostRange`] / [`SearchHit`]）；
//! - 读写走单一端口 [`ForumRepo`]（建/列/读/追加/搜索/状态/归档），实现在载体
//!   crate `pipi-forum`（纯文件）。换载体（例如 git 对象后端）只换实现，调用点
//!   与 DTO 不变。
//!
//! **身份由调用方提供**（工具路径来自运行上下文，CLI 路径来自 `--author`），
//! 端口不做身份判断 —— 这是「模型不得伪造作者」这条不变量的落点：装配工具时
//! 由宿主盖章，见设计文档。

use serde::{Deserialize, Serialize};

/// 一次写入的来源，用于审计「谁写的、怎么写的」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PostVia {
    /// Agent 通过原生论坛工具写入（身份由运行时盖章）。
    Tool,
    /// 人在命令行写入。
    Cli,
    /// 人在 Web/桌面界面写入。
    Web,
    /// 未知 / 未来版本写入的来源。**缺省值与向前兼容兜底**：手写文件不写 `via`
    /// 时**绝不**默认成 `Tool`（否则可伪造「最受信任的通道」）。
    #[default]
    #[serde(other)]
    Other,
}

/// 主题帖状态。
///
/// `locked`（只读、区别于关闭）若引入，会作为**独立的 bool 字段**而不是本枚举的
/// 新变体（见设计文档「状态模型」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadStatus {
    #[default]
    Open,
    Closed,
    /// 未知 / 未来版本写入的状态。**向前兼容的兜底变体**：旧读者遇到
    /// `settled` 之类的新状态时降级为它（主题仍可见），而不是让主题从列表里消失。
    #[serde(other)]
    Unknown,
}

impl ThreadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

/// 主题帖元数据（`thread.toml`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// `{millis}-{8hex}`，同时是目录名（[`validate_forum_id`] 保证路径安全）。
    pub id: String,
    pub title: String,
    /// 起帖作者（Agent 名或人类标识）。
    pub author: String,
    pub created: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: ThreadStatus,
    #[serde(default)]
    pub pinned: bool,
}

/// 一条帖子（`posts/<seq>-<id>.md` 的 frontmatter + 正文）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    /// `{millis}-{8hex}`，回复用它做 `reply_to`。
    pub id: String,
    /// 主题内序号（从 1 递增），决定展示顺序。
    pub seq: u64,
    pub author: String,
    pub created: u64,
    #[serde(default)]
    pub via: PostVia,
    /// 回复的目标帖子 id（顶层帖为 `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// `@提及` 的 Agent 名（由调用方校验存在性）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    /// 根帖 id：本主题内因果链的起点（顶层帖 = 自身 `id`）。写时确定、永不变化，
    /// 是 Phase 2 护栏（最大因果深度）的落点。旧文件缺失时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<String>,
    /// 因果深度：根帖 0，直接回复根帖 1，依此类推。旧文件缺失时为 0。
    #[serde(default)]
    pub causality_depth: u32,
    /// 内容哈希（**载体计算**）：`sha256:<hex>`。同一主题内 `(author, reply_to, content_hash)`
    /// 相同即视为重复写（重试 / 重放），`append_post` 会返回已有帖而不新建。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// 来源标记（**调用方**给的溯源 token，例如工具路径的 `run:<runId>`）。审计用；
    /// Phase 2 派发器的幂等键会用到。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 正文（Markdown）。
    pub body: String,
}

impl Post {
    /// 这条帖子是否 `@提及` 了某 Agent。
    pub fn mentions_agent(&self, agent: &str) -> bool {
        self.mentions.iter().any(|name| name == agent)
    }
}

/// 列表用的主题摘要（元数据 + 派生统计）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub thread: Thread,
    pub post_count: usize,
    /// 最后一条帖子的时间（空帖回退到 `thread.created`）。
    pub last_active: u64,
    pub last_author: Option<String>,
}

/// 新建主题的草稿（首帖含在 `body` 里）。
#[derive(Debug, Clone)]
pub struct NewThread {
    pub title: String,
    pub author: String,
    pub body: String,
    pub tags: Vec<String>,
    pub mentions: Vec<String>,
    pub via: PostVia,
    /// 可选的来源标记（工具路径写 `run:<sessionId>:<runId>`）；审计与幂等用。
    pub origin: Option<String>,
}

/// 回帖的草稿。
#[derive(Debug, Clone)]
pub struct NewPost {
    pub author: String,
    pub body: String,
    pub via: PostVia,
    /// 回复的目标帖子 id；`None` 表示回复主题本身。
    pub reply_to: Option<String>,
    pub mentions: Vec<String>,
    /// 可选的来源标记（工具路径写 `run:<runId>`）；审计用。`content_hash` 由载体计算。
    pub origin: Option<String>,
}

/// 列表过滤条件。
#[derive(Debug, Clone, Default)]
pub struct ThreadFilter {
    /// `Some` 时只返回该状态；`None` = 不限。
    pub status: Option<ThreadStatus>,
    /// 只返回带该标签的主题。
    pub tag: Option<String>,
    /// 是否包含归档区（默认只看活跃区）。
    pub include_archived: bool,
}

/// 帖子读取范围（分页）。
#[derive(Debug, Clone, Copy, Default)]
pub struct PostRange {
    pub offset: usize,
    /// `None` = 不限条数。
    pub limit: Option<usize>,
}

/// 一条检索命中（每个主题最多一处，对齐 `session_search` 的防浪费口径）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub thread_id: String,
    pub thread_title: String,
    pub post_id: String,
    pub post_seq: u64,
    pub author: String,
    /// 命中位置附近的截断片段。
    pub snippet: String,
}

/// 检索选项。
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    /// 最多返回多少个主题（每个主题最多一处）。
    pub limit: usize,
    /// 是否同时检索归档区（默认只看活跃区）。
    pub include_archived: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            include_archived: false,
        }
    }
}

/// 论坛存储端口：按版面根目录构造，目录级操作（建/列/读/追加/搜索/状态/归档）。
///
/// 运行时的论坛读写全部走这里 —— 换载体只换实现，调用点不动。实现必须对符号
/// 链接与路径穿越 fail-closed（主题 id 只接受 [`validate_forum_id`] 放行的形状）。
pub trait ForumRepo: Send + Sync {
    /// 新建一个主题（含首帖），返回其元数据。
    fn create_thread(&self, draft: &NewThread) -> Result<Thread, String>;
    /// 读取主题元数据。
    fn thread(&self, thread_id: &str) -> Result<Thread, String>;
    /// 列主题摘要，按「置顶优先、最后活跃倒序」排序，最多 `limit` 条。
    fn list_threads(
        &self,
        filter: &ThreadFilter,
        limit: usize,
    ) -> Result<Vec<ThreadSummary>, String>;
    /// 读主题内的帖子（按序号升序，应用 `range`）。
    fn posts(&self, thread_id: &str, range: PostRange) -> Result<Vec<Post>, String>;
    /// 追加一条帖子，返回落盘后的样子（含分配的 id/seq/时间）。
    fn append_post(&self, thread_id: &str, draft: &NewPost) -> Result<Post, String>;
    /// 全文子串检索（大小写不敏感，匹配标题与正文），每个主题最多一处命中。
    fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchHit>, String>;
    /// 设置主题状态（open / closed）。
    fn set_status(&self, thread_id: &str, status: ThreadStatus) -> Result<(), String>;
    /// 置顶 / 取消置顶。
    fn set_pinned(&self, thread_id: &str, pinned: bool) -> Result<(), String>;
    /// 移入归档区。
    fn archive(&self, thread_id: &str) -> Result<(), String>;
    /// 从归档区移回活跃区。
    fn restore(&self, thread_id: &str) -> Result<(), String>;
}

/// 论坛 id 校验：只允许 ASCII 字母数字与 `-`（主题目录名与帖子 id 共用）。
///
/// 与 [`crate::session::validate_session_id`] 同款：id 会进文件路径，必须拒绝
/// 路径分隔符、`.` 与空串。
pub fn validate_forum_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("Invalid forum id".into());
    }
    Ok(())
}

/// 提及标名校验：trim 后非空、无控制字符、无路径分隔符（存在性由调用方另行校验）。
pub fn validate_mention(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Mention cannot be empty".into());
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(format!("Invalid mention name: {name:?}"));
    }
    Ok(())
}

/// Agent 名字符集（与 `pipi_venv::agents::validate_agent_name` 同口径）：
/// ASCII 字母数字与 `-` / `_`。
fn is_name_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
}

/// 从正文里解析 `@name` 提及。
///
/// **正文 token 是提及的权威来源之一**：人机共写下，人会直接在正文里 `@builder`，
/// 写入时由载体与显式 `mentions` 合并落库。规则：
///
/// - 名字字符集与 Agent 名一致（ASCII 字母数字 + `-` / `_`）；
/// - `@` 前一个字符必须是行首或**非名字字符**（避免 `a@b.com` 这类误判）；
/// - 跳过 ``` 围栏代码块内的内容；
/// - 去重、保持出现顺序。
///
/// **不**判断名字是否真实存在（那是调用方查注册表的事），也不排除自己。
pub fn parse_mentions(body: &str) -> Vec<String> {
    let mut mentions: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            let at_word_boundary =
                chars[index] == '@' && !(index > 0 && is_name_char(chars[index - 1]));
            if at_word_boundary {
                let start = index + 1;
                let mut end = start;
                while end < chars.len() && is_name_char(chars[end]) {
                    end += 1;
                }
                if end > start {
                    let name: String = chars[start..end].iter().collect();
                    if !mentions.contains(&name) {
                        mentions.push(name);
                    }
                    index = end;
                    continue;
                }
            }
            index += 1;
        }
    }
    mentions
}

/// 合并显式提及与正文解析出的提及（显式在前），去重、保持顺序。
pub fn merged_mentions(explicit: &[String], body: &str) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for name in explicit {
        let name = name.trim();
        if !name.is_empty() && !merged.iter().any(|existing| existing == name) {
            merged.push(name.to_string());
        }
    }
    for name in parse_mentions(body) {
        if !merged.iter().any(|existing| existing == &name) {
            merged.push(name);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forum_ids_reject_traversal_and_empty() {
        assert!(validate_forum_id("1758000000000-ab12cd34").is_ok());
        for invalid in ["", ".", "..", "../escape", "nested/name", "a.b", "会话"] {
            assert!(
                validate_forum_id(invalid).is_err(),
                "accepted invalid id: {invalid}"
            );
        }
    }

    #[test]
    fn mentions_reject_control_separators_and_blank() {
        assert!(validate_mention("reviewer").is_ok());
        assert!(validate_mention("builder-2").is_ok());
        for invalid in ["", "   ", "a/b", "a\\b", "bad\nname"] {
            assert!(validate_mention(invalid).is_err(), "accepted: {invalid:?}");
        }
    }

    #[test]
    fn post_mentions_agent_is_exact_match() {
        let post = Post {
            id: "x".into(),
            seq: 1,
            author: "a".into(),
            created: 0,
            via: PostVia::Cli,
            reply_to: None,
            mentions: vec!["builder".into()],
            root_id: None,
            causality_depth: 0,
            content_hash: None,
            origin: None,
            body: String::new(),
        };
        assert!(post.mentions_agent("builder"));
        assert!(!post.mentions_agent("build"));
        assert!(!post.mentions_agent("Builder"));
    }

    /// 向前兼容：未知枚举值降级为兜底变体，而不是让整条记录失败。
    #[test]
    fn unknown_enum_values_fall_back_instead_of_failing() {
        assert_eq!(
            serde_json::from_str::<ThreadStatus>("\"settled\"").unwrap(),
            ThreadStatus::Unknown
        );
        assert_eq!(
            serde_json::from_str::<PostVia>("\"system\"").unwrap(),
            PostVia::Other
        );
    }

    #[test]
    fn parse_mentions_respects_boundaries_and_code_fences() {
        // 词边界：`a@b.com` 不算提及，`(@builder` 算。
        assert_eq!(parse_mentions("mail a@b.com 和 (@builder"), vec!["builder"]);
        // 名字字符集：`.` 终止名字。
        assert_eq!(parse_mentions("@planner. 你好"), vec!["planner"]);
        // 去重保序。
        assert_eq!(
            parse_mentions("@b @a @b"),
            vec!["b".to_string(), "a".to_string()]
        );
        // 代码块内不解析。
        assert_eq!(
            parse_mentions("外面 @real\n```\n@ghost\n```\n后面 @also"),
            vec!["real".to_string(), "also".to_string()]
        );
        // 行首也生效。
        assert_eq!(parse_mentions("@start"), vec!["start"]);
    }

    #[test]
    fn merged_mentions_puts_explicit_first_and_dedups() {
        let explicit = vec![" planner ".to_string(), "builder".to_string()];
        assert_eq!(
            merged_mentions(&explicit, "@builder @reviewer"),
            vec![
                "planner".to_string(),
                "builder".to_string(),
                "reviewer".to_string()
            ]
        );
    }
}
