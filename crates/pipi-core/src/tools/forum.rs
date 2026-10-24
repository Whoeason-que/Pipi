//! 论坛工具（显式启用）：让 Agent 参与本地论坛。
//!
//! 事实源是一个纯文件版面（默认 `~/.pipi/forum`，工作区里带 `.agents/forum`
//! 则用本地的 —— 根目录由 `crate::forum::resolve_root` 解析，与 CLI 同口径）。
//! 本模块只做「把模型给的参数变成一个 `ForumRepo` 调用」，存储细节在
//! `pipi-forum` 载体。
//!
//! **身份盖章**：`author` 来自构造时注入的 Agent 名，模型无法指定 —— 这是
//! 「工具路径作者不可伪造」的落点（CLI 路径的作者是自报，故 Phase 2 的路由
//! 只信任工具写入的帖）。`via` 恒为 `tool`。
//!
//! 四个工具都要显式启用（`FORUM_TOOLS`），不随基础工具默认开放：它们读写一个
//! 跨 Agent 的共享版面，属于组合能力。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use pipi_protocol::ToolResultContent;
use pipi_protocol::forum::{
    ForumRepo, NewPost, NewThread, Post, PostRange, PostVia, Thread, ThreadFilter, ThreadStatus,
};
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};

/// 单次工具输出的字符预算（对齐 `session_search` 的防浪费口径）。
const MAX_OUTPUT_CHARS: usize = 8_000;
/// 列表默认/上限主题数。
const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 50;

/// 列出论坛主题。
pub struct ForumListTool {
    repo: Arc<dyn ForumRepo>,
}

impl ForumListTool {
    pub fn new(repo: Arc<dyn ForumRepo>) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl AgentTool for ForumListTool {
    fn name(&self) -> &'static str {
        "forum_list"
    }

    fn description(&self) -> String {
        "List threads on the shared local forum — a file-backed discussion board that other agents \
         and the human can read and write. Optional `status` (open/closed), `tag` and \
         `includeArchived` filters. Use this to see what is being discussed before posting."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "enum": ["open", "closed"], "description": "Only threads with this status." },
                "tag": { "type": "string", "description": "Only threads carrying this tag." },
                "includeArchived": { "type": "boolean", "description": "Include archived threads (default false)." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT, "description": "Max threads (default 20)." }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let filter = ThreadFilter {
            status: parse_status(args.get("status"))?,
            tag: args.get("tag").and_then(Value::as_str).map(str::to_string),
            include_archived: args
                .get("includeArchived")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT);
        let threads = self.repo.list_threads(&filter, limit)?;
        let text = render_list(&threads);
        Ok(text_output(
            text,
            serde_json::to_value(&threads).map_err(|e| e.to_string())?,
        ))
    }
}

/// 读取一个主题的帖子。
///
/// 读取即消费：会把该主题标记为「读到最后一帖」，供下一次 run 的未读摘要使用。
pub struct ForumReadTool {
    repo: Arc<dyn ForumRepo>,
    root: PathBuf,
    agent: String,
}

impl ForumReadTool {
    pub fn new(repo: Arc<dyn ForumRepo>, root: PathBuf, agent: String) -> Self {
        Self { repo, root, agent }
    }
}

#[async_trait]
impl AgentTool for ForumReadTool {
    fn name(&self) -> &'static str {
        "forum_read"
    }

    fn description(&self) -> String {
        "Read a forum thread's posts (thread metadata + all posts, oldest first). Pass the \
         `threadId` from forum_list. Use `offset` to page through long threads."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "threadId": { "type": "string", "description": "Thread id from forum_list." },
                "offset": { "type": "integer", "minimum": 0, "description": "Index of the first post to return (default 0)." },
                "limit": { "type": "integer", "minimum": 1, "description": "Max posts to return." }
            },
            "required": ["threadId"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let thread_id = required_str(args, "threadId")?;
        let thread = self.repo.thread(thread_id)?;
        let range = PostRange {
            offset: args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
        };
        let posts = self.repo.posts(thread_id, range)?;
        let (text, last_visible) = render_read(&thread, &posts);
        // 读取即消费，但只推进到**确实渲染进模型可见输出**的最后一帖；
        // 被中止的 run 不推进（与「失败/中断不推进」一致）。
        if !ctx.abort.is_aborted()
            && let Some(id) = last_visible
        {
            crate::forum::mark_read(&self.root, &self.agent, thread_id, &id);
        }
        let details = json!({ "thread": thread, "posts": posts });
        Ok(text_output(text, details))
    }
}

/// 发起一个新主题。
pub struct ForumPostTool {
    repo: Arc<dyn ForumRepo>,
    author: String,
    /// 来源标记（`run:<sessionId>:<runId>`），落到首帖的 `origin` 上并参与幂等。
    origin: String,
}

impl ForumPostTool {
    pub fn new(repo: Arc<dyn ForumRepo>, author: String, origin: String) -> Self {
        Self {
            repo,
            author,
            origin,
        }
    }
}

#[async_trait]
impl AgentTool for ForumPostTool {
    fn name(&self) -> &'static str {
        "forum_post"
    }

    fn description(&self) -> String {
        "Start a new thread on the shared local forum. Your agent name is recorded as the author. \
         Writing `@name` in the body mentions that agent (it is stored as a mention). Prefer \
         replying to an existing thread when the topic already exists."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Thread title." },
                "body": { "type": "string", "description": "First post, Markdown. `@name` mentions are extracted." },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags." },
                "mentions": { "type": "array", "items": { "type": "string" }, "description": "Optional extra agent names to mention." }
            },
            "required": ["title", "body"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let draft = NewThread {
            title: required_str(args, "title")?.to_string(),
            author: self.author.clone(),
            body: required_str(args, "body")?.to_string(),
            tags: string_array(args.get("tags"))?,
            mentions: string_array(args.get("mentions"))?,
            via: PostVia::Tool,
            origin: Some(self.origin.clone()),
        };
        let thread = self.repo.create_thread(&draft)?;
        let text = format!("Created thread {}: \"{}\"", thread.id, thread.title);
        Ok(text_output(
            text,
            serde_json::to_value(&thread).map_err(|e| e.to_string())?,
        ))
    }
}

/// 回复一个主题。
pub struct ForumReplyTool {
    repo: Arc<dyn ForumRepo>,
    author: String,
    origin: String,
}

impl ForumReplyTool {
    pub fn new(repo: Arc<dyn ForumRepo>, author: String, origin: String) -> Self {
        Self {
            repo,
            author,
            origin,
        }
    }
}

#[async_trait]
impl AgentTool for ForumReplyTool {
    fn name(&self) -> &'static str {
        "forum_reply"
    }

    fn description(&self) -> String {
        "Reply to a forum thread. Your agent name is recorded as the author. Pass `replyTo` (a post \
         id from forum_read) to reply to a specific post; writing `@name` in the body mentions that \
         agent. Closed or archived threads reject replies."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "threadId": { "type": "string", "description": "Thread id from forum_list." },
                "body": { "type": "string", "description": "Reply body, Markdown. `@name` mentions are extracted." },
                "replyTo": { "type": "string", "description": "Optional post id to reply to." },
                "mentions": { "type": "array", "items": { "type": "string" }, "description": "Optional extra agent names to mention." }
            },
            "required": ["threadId", "body"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let thread_id = required_str(args, "threadId")?;
        let draft = NewPost {
            author: self.author.clone(),
            body: required_str(args, "body")?.to_string(),
            via: PostVia::Tool,
            reply_to: args
                .get("replyTo")
                .and_then(Value::as_str)
                .map(str::to_string),
            mentions: string_array(args.get("mentions"))?,
            origin: Some(self.origin.clone()),
        };
        let post = self.repo.append_post(thread_id, &draft)?;
        let text = format!("Replied post #{} to thread {thread_id}", post.seq);
        Ok(text_output(
            text,
            serde_json::to_value(&post).map_err(|e| e.to_string())?,
        ))
    }
}

fn text_output(text: String, details: Value) -> ToolOutput {
    ToolOutput {
        content: vec![ToolResultContent::Text { text }],
        details: Some(details),
        terminate: false,
    }
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Missing {key}"))
}

fn parse_status(value: Option<&Value>) -> Result<Option<ThreadStatus>, String> {
    match value.and_then(Value::as_str) {
        None => Ok(None),
        Some("open") => Ok(Some(ThreadStatus::Open)),
        Some("closed") => Ok(Some(ThreadStatus::Closed)),
        Some(other) => Err(format!("status must be open or closed (got {other:?})")),
    }
}

fn string_array(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "Array element must be a string".to_string())
            })
            .collect(),
        Some(_) => Err("Must be an array of strings".into()),
    }
}

fn render_list(threads: &[pipi_protocol::forum::ThreadSummary]) -> String {
    if threads.is_empty() {
        return "Forum has no threads yet.".into();
    }
    let mut lines = vec![format!("Forum threads ({}):", threads.len())];
    for summary in threads {
        let thread = &summary.thread;
        let tags = if thread.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", thread.tags.join(","))
        };
        let pin = if thread.pinned { "📌 " } else { "" };
        let author = summary.last_author.as_deref().unwrap_or(&thread.author);
        lines.push(format!(
            "- {pin}{}  [{}]  {}{}  ({} posts, last {} by {})",
            thread.id,
            thread.status.as_str(),
            thread.title,
            tags,
            summary.post_count,
            summary.last_active,
            author,
        ));
    }
    lines.join("\n")
}

/// 渲染主题内容，返回 `(文本, 确实渲染进文本的最后一帖 id)`。
///
/// 第二个值用于「读取即消费」：只有出现在模型可见输出里的帖子才算已读。
fn render_read(thread: &Thread, posts: &[Post]) -> (String, Option<String>) {
    let mut out = format!(
        "# {}  [{}]  {}\nStarted by {} @ {}\n",
        thread.title,
        thread.status.as_str(),
        thread.id,
        thread.author,
        thread.created
    );
    let mut used = out.chars().count();
    if posts.is_empty() {
        out.push_str("(This thread has no posts yet)\n");
        return (out, None);
    }
    let mut last_visible: Option<String> = None;
    let mut omitted = 0usize;
    for post in posts {
        let mut header = format!("#{} {}", post.seq, post.author);
        if let Some(parent) = &post.reply_to {
            header.push_str(&format!(" ↩ {parent}"));
        }
        if !post.mentions.is_empty() {
            header.push_str(&format!(" → {}", post.mentions.join(", ")));
        }
        let block = format!("─────\n{header}\n{}\n", post.body);
        if used + block.chars().count() > MAX_OUTPUT_CHARS {
            omitted += 1;
            continue;
        }
        used += block.chars().count();
        last_visible = Some(post.id.clone());
        out.push_str(&block);
    }
    if omitted > 0 {
        out.push_str(&format!(
            "(Output reached budget, {omitted} posts omitted; use offset to continue reading)\n"
        ));
    }
    (out, last_visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::AbortSignal;
    use pipi_protocol::permissions::PermissionsConfig;
    use pipi_protocol::permissions::SandboxMode;
    use std::collections::BTreeMap;

    fn context() -> ToolContext {
        ToolContext {
            workspace: pipi_test_support::temp_dir("pipi-core-forum-tool"),
            memory_dir: None,
            read_roots: Vec::new(),
            permissions: Arc::new(PermissionsConfig::default()),
            sandbox: SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
            approver: None,
        }
    }

    fn repo() -> (Arc<dyn ForumRepo>, std::path::PathBuf) {
        let root = pipi_test_support::temp_dir("pipi-core-forum-tool-root");
        let repo: Arc<dyn ForumRepo> =
            Arc::new(pipi_forum::FileForumRepo::open(root.clone()).unwrap());
        (repo, root)
    }

    #[tokio::test]
    async fn post_list_read_and_reply_roundtrip() {
        let (repo, root) = repo();
        let ctx = context();
        let noop = |_: ToolOutput| {};

        let posted = ForumPostTool::new(repo.clone(), "planner".into(), "run:1".into())
            .execute(
                &ctx,
                &json!({ "title": "并发模型", "body": "请 @builder 看看" }),
                &noop,
            )
            .await
            .unwrap();
        let thread_id = posted.details.unwrap()["id"].as_str().unwrap().to_string();

        let listed = ForumListTool::new(repo.clone())
            .execute(&ctx, &json!({}), &noop)
            .await
            .unwrap();
        let list_text = match &listed.content[0] {
            ToolResultContent::Text { text } => text.clone(),
            _ => unreachable!(),
        };
        assert!(list_text.contains("并发模型"), "{list_text}");
        assert!(list_text.contains("planner"), "{list_text}");

        // 作者由工具盖章（模型无法指定），正文里的 @builder 被解析为提及。
        let read = ForumReadTool::new(repo.clone(), root.clone(), "planner".into())
            .execute(&ctx, &json!({ "threadId": thread_id }), &noop)
            .await
            .unwrap();
        let details = read.details.unwrap();
        assert_eq!(details["posts"][0]["author"], "planner");
        assert_eq!(details["posts"][0]["mentions"][0], "builder");
        // 读取即消费：游标落在最后一帖上。
        assert_eq!(
            crate::forum::read_cursor(&root, "planner")
                .get(&thread_id)
                .map(String::as_str),
            Some(details["posts"][0]["id"].as_str().unwrap())
        );

        let replied = ForumReplyTool::new(repo.clone(), "builder".into(), "run:2".into())
            .execute(
                &ctx,
                &json!({ "threadId": thread_id, "body": "收到" }),
                &noop,
            )
            .await
            .unwrap();
        assert_eq!(replied.details.unwrap()["seq"], 2);
    }

    #[tokio::test]
    async fn read_rejects_dangling_reply_and_bad_status() {
        let (repo, root) = repo();
        let ctx = context();
        let noop = |_: ToolOutput| {};
        let thread = repo
            .create_thread(&NewThread {
                title: "t".into(),
                author: "a".into(),
                body: "请 @b 看下".into(),
                tags: vec![],
                mentions: vec![],
                via: PostVia::Tool,
                origin: None,
            })
            .unwrap();
        let err = ForumReplyTool::new(repo.clone(), "b".into(), "run:1".into())
            .execute(
                &ctx,
                &json!({ "threadId": thread.id, "body": "x", "replyTo": "nope" }),
                &noop,
            )
            .await
            .unwrap_err();
        assert!(
            err.contains("non-existent") || err.contains("not exist"),
            "{err}"
        );
        assert!(
            ForumListTool::new(repo.clone())
                .execute(&ctx, &json!({ "status": "bogus" }), &noop)
                .await
                .is_err()
        );
        // 未读摘要：新帖提到该 Agent 时，他能在下一次 run 前看到提醒。
        let digest = crate::forum::digest(repo.as_ref(), &root, "b").unwrap();
        assert!(digest.is_some(), "被 @ 的 Agent 应看到未读摘要");
        // 读完后不再提醒。
        crate::forum::mark_read(
            &root,
            "b",
            &thread.id,
            repo.posts(&thread.id, PostRange::default())
                .unwrap()
                .last()
                .unwrap()
                .id
                .as_str(),
        );
        assert!(
            crate::forum::digest(repo.as_ref(), &root, "b")
                .unwrap()
                .is_none()
        );
    }
}
