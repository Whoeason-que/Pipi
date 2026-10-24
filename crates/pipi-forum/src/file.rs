//! 纯文件载体 [`FileForumRepo`]：一个主题一个目录、一条帖子一个文件。
//!
//! 与会话载体同款理由：只追加、git 友好。写序列化由一把进程内写锁保证
//! （`write_lock`）：**所有**结构性写操作（建主题、回帖、改状态、置顶、归档、
//! 恢复）都在同一把锁下，「检查→写入」构成临界区，因此不存在「关帖后仍被回帖」
//! 或「归档与回帖互相撕裂」的窗口。跨进程（桌面壳与 Web 宿主同开一个版面）没有
//! 写者锁，只靠 `O_EXCL` 防文件名相撞 —— 这是已知限制，见设计文档。
//!
//! **路径安全 fail-closed**：`threads/`、`.archive/`、主题目录逐级走
//! [`pipi_protocol::fs::ensure_real_directory`]（符号链接即报错），帖子文件用
//! `file_type()`（不跟随符号链接）过滤，`thread.toml` 走 `ensure_real_file`。
//!
//! **未知字段不丢**：`thread.toml` 的更新（改状态/置顶）在 TOML 表上原地合并，
//! 而非「读成结构体再整体重写」，所以未来版本写入的 `board` / `updated_at` 等
//! 字段不会被旧版本的回写吃掉。帖子文件从不回写。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use pipi_protocol::forum::{
    ForumRepo, NewPost, NewThread, Post, PostRange, SearchHit, SearchOptions, Thread, ThreadFilter,
    ThreadStatus, ThreadSummary, merged_mentions, validate_forum_id,
};

use crate::format::{
    decode_post, decode_thread, encode_post, encode_thread, find_case_insensitive, new_forum_id,
    post_filename,
};

/// 检索片段的字符上限（对齐 `session_search`）。
const MAX_SNIPPET_CHARS: usize = 240;

/// 纯文件论坛载体。
pub struct FileForumRepo {
    root: PathBuf,
    /// 结构性写的串行点（同进程内）。见模块文档。
    write_lock: Mutex<()>,
}

impl FileForumRepo {
    /// 打开（必要时创建）一个版面根目录。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        if !pipi_protocol::fs::ensure_real_directory(&root, "论坛根目录")? {
            fs::create_dir_all(&root).map_err(|error| format!("无法创建论坛根目录: {error}"))?;
            pipi_protocol::fs::ensure_real_directory(&root, "论坛根目录")?;
        }
        Ok(Self {
            root,
            write_lock: Mutex::new(()),
        })
    }

    /// 初始化一个论坛根目录（显式入口，供 `pipi-forum init` 调用）：
    /// 建 `threads/`、`.archive/` 与标记文件 `forum.toml`。幂等（已存在不覆盖）。
    pub fn init(root: impl Into<PathBuf>) -> Result<PathBuf, String> {
        let repo = Self::open(root)?;
        repo.ensured_real_dir(&repo.threads_path(), "论坛主题目录")?;
        repo.ensured_real_dir(&repo.archive_path(), "归档目录")?;
        let marker = repo.root.join(crate::root::FORUM_MARKER);
        if !pipi_protocol::fs::ensure_real_file(&marker, "论坛标记文件")? {
            pipi_protocol::fs::write_atomic(&marker, crate::root::MARKER_CONTENT)
                .map_err(|error| format!("写入论坛标记失败: {error}"))?;
        }
        Ok(repo.root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.write_lock
            .lock()
            .map_err(|error| format!("论坛写锁不可用: {error}"))
    }

    fn threads_path(&self) -> PathBuf {
        self.root.join("threads")
    }

    fn archive_path(&self) -> PathBuf {
        self.root.join(".archive")
    }

    /// 已存在的真实目录；不存在返回 `None`；符号链接/非目录返回 `Err`。
    fn existing_real_dir(&self, path: &Path, label: &str) -> Result<Option<PathBuf>, String> {
        if pipi_protocol::fs::ensure_real_directory(path, label)? {
            Ok(Some(path.to_path_buf()))
        } else {
            Ok(None)
        }
    }

    /// 确保真实目录存在（必要时创建）；符号链接照样 fail-closed。
    fn ensured_real_dir(&self, path: &Path, label: &str) -> Result<PathBuf, String> {
        if pipi_protocol::fs::ensure_real_directory(path, label)? {
            return Ok(path.to_path_buf());
        }
        fs::create_dir_all(path)
            .map_err(|error| format!("无法创建{label} {}: {error}", path.display()))?;
        pipi_protocol::fs::ensure_real_directory(path, label)?;
        Ok(path.to_path_buf())
    }

    /// 定位一个主题目录：先活跃区，再归档区。返回 `(目录, 是否已归档)`。
    ///
    /// 逐级检查真实目录：先验区目录本身（挡住 `threads` 是符号链接的情况），
    /// 再验主题目录。
    fn locate(&self, thread_id: &str) -> Result<(PathBuf, bool), String> {
        validate_forum_id(thread_id)?;
        if let Some(threads) = self.existing_real_dir(&self.threads_path(), "论坛主题目录")?
            && let Some(dir) = self.existing_real_dir(&threads.join(thread_id), "主题目录")?
        {
            return Ok((dir, false));
        }
        if let Some(archive) = self.existing_real_dir(&self.archive_path(), "归档目录")?
            && let Some(dir) = self.existing_real_dir(&archive.join(thread_id), "归档主题目录")?
        {
            return Ok((dir, true));
        }
        Err(format!("Thread \"{thread_id}\" does not exist"))
    }

    /// 枚举一个区里的主题目录 id（`file_type()` 不跟随符号链接，链接条目被跳过）。
    fn thread_ids_in(&self, dir: &Path) -> Result<Vec<String>, String> {
        let mut ids = Vec::new();
        let entries = fs::read_dir(dir)
            .map_err(|error| format!("Cannot read forum directory {}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("枚举论坛目录失败: {error}"))?;
            let is_real_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if !is_real_dir {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if validate_forum_id(&name).is_ok() {
                ids.push(name);
            }
        }
        Ok(ids)
    }

    /// 读一个主题目录下的全部帖子，按 `(seq, id)` 升序。
    ///
    /// 单个条目的任何问题（符号链接、目录、非 UTF-8、坏 frontmatter）都**跳过**，
    /// 不再让一个坏文件 brick 整个主题。
    fn read_posts_in(&self, thread_dir: &Path) -> Result<Vec<Post>, String> {
        let Some(posts_dir) = self.existing_real_dir(&thread_dir.join("posts"), "帖子目录")?
        else {
            return Ok(Vec::new());
        };
        let entries = fs::read_dir(&posts_dir)
            .map_err(|error| format!("无法读取帖子目录 {}: {error}", posts_dir.display()))?;
        let mut posts = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("枚举帖子目录失败: {error}"))?;
            // file_type 不跟随符号链接：链接与目录都被跳过。
            if !entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if !file_name.ends_with(".md") {
                continue;
            }
            let Ok(content) = fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Ok(post) = decode_post(&content) {
                posts.push(post);
            }
        }
        posts.sort_by(|a, b| a.seq.cmp(&b.seq).then_with(|| a.id.cmp(&b.id)));
        Ok(posts)
    }

    /// 在指定主题目录追一条帖子（**调用方必须已持有写锁**）。
    fn append_post_in(&self, thread_dir: &Path, draft: &NewPost) -> Result<Post, String> {
        if draft.body.trim().is_empty() {
            return Err("Post body cannot be empty".into());
        }
        let author = draft.author.trim().to_string();
        validate_author(&author)?;
        // 正文里的 `@name` 与显式 mentions 合并（正文 token 也是权威来源），并排除「自己 @ 自己」。
        let mut mentions = merged_mentions(&draft.mentions, &draft.body);
        mentions.retain(|name| name != &author);
        for mention in &mentions {
            pipi_protocol::forum::validate_mention(mention)?;
        }
        let posts_dir = self.ensured_real_dir(&thread_dir.join("posts"), "posts directory")?;
        let existing = self.read_posts_in(thread_dir)?;
        let content_hash = content_hash(&draft.body);

        // 因果链：解析并校验 reply_to 指向本主题内存在的帖子。
        let (reply_to, root_id, causality_depth) = match draft.reply_to.as_deref() {
            None => (None, None, 0),
            Some(parent_id) => {
                let parent = existing
                    .iter()
                    .find(|post| post.id == parent_id)
                    .ok_or_else(|| {
                        format!("reply_to points to non-existent post in this thread: {parent_id}")
                    })?;
                let root = parent.root_id.clone().unwrap_or_else(|| parent.id.clone());
                (
                    Some(parent_id.to_string()),
                    Some(root),
                    parent.causality_depth + 1,
                )
            }
        };

        // 幂等：**只对带显式 origin 的请求（工具重试）去重**；人写（origin=None）不去重，
        // 避免吞掉合法重复。键含 origin / 作者 / 父帖 / 提及 / 内容，形状不同的请求不归并。
        if let Some(origin) = draft.origin.as_deref()
            && let Some(existing_post) = existing.iter().find(|post| {
                post.origin.as_deref() == Some(origin)
                    && post.author == author
                    && post.reply_to == reply_to
                    && post.mentions == mentions
                    && same_content(post, &content_hash, &draft.body)
            })
        {
            return Ok(existing_post.clone());
        }

        let mut seq = existing.iter().map(|post| post.seq).max().unwrap_or(0) + 1;
        loop {
            let id = new_forum_id();
            let post = Post {
                id: id.clone(),
                seq,
                author: author.clone(),
                created: pipi_protocol::now_millis(),
                via: draft.via,
                reply_to: reply_to.clone(),
                mentions: mentions.clone(),
                // 顶层帖的根是它自己。
                root_id: root_id.clone().or(Some(id)),
                causality_depth,
                content_hash: Some(content_hash.clone()),
                origin: draft.origin.clone(),
                body: draft.body.clone(),
            };
            let path = posts_dir.join(post_filename(&post));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    file.write_all(encode_post(&post).as_bytes())
                        .map_err(|error| format!("写入帖子失败: {error}"))?;
                    return Ok(post);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    seq += 1;
                    continue;
                }
                Err(error) => return Err(format!("Failed to create post file: {error}")),
            }
        }
    }

    /// 读一个主题目录的元数据（`thread.toml` 必须是真实普通文件）。
    fn read_thread_in(&self, thread_dir: &Path) -> Result<Thread, String> {
        let path = thread_dir.join("thread.toml");
        if !pipi_protocol::fs::ensure_real_file(&path, "thread metadata")? {
            return Err(format!("Thread is missing thread.toml: {}", path.display()));
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        decode_thread(&content)
    }

    /// 新建时写入 `thread.toml`（全字段）。
    fn write_thread_in(&self, thread_dir: &Path, thread: &Thread) -> Result<(), String> {
        let path = thread_dir.join("thread.toml");
        pipi_protocol::fs::write_atomic(&path, encode_thread(thread))
            .map_err(|error| format!("Failed to write {}: {error}", path.display()))
    }

    /// 在 TOML 表上原地更新 `thread.toml`（保留未知字段），用于改状态 / 置顶。
    fn patch_thread_in(
        &self,
        thread_dir: &Path,
        patch: impl FnOnce(&mut toml::Table),
    ) -> Result<(), String> {
        let path = thread_dir.join("thread.toml");
        if !pipi_protocol::fs::ensure_real_file(&path, "thread metadata")? {
            return Err(format!("Thread is missing thread.toml: {}", path.display()));
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        let mut table: toml::Table = toml::from_str(&content)
            .map_err(|error| format!("Failed to parse thread metadata: {error}"))?;
        patch(&mut table);
        let text = toml::to_string(&table)
            .map_err(|error| format!("Failed to serialize thread metadata: {error}"))?;
        pipi_protocol::fs::write_atomic(&path, text)
            .map_err(|error| format!("Failed to write {}: {error}", path.display()))
    }

    /// 找一个「同作者 + 同标题 + 首帖内容相同」的已有主题（新建时的幂等判据）。
    fn find_duplicate_thread(&self, draft: &NewThread) -> Result<Option<Thread>, String> {
        // 只对带显式 origin 的请求（工具重试）生效；人写（origin=None）不去重。
        let Some(origin) = draft.origin.as_deref() else {
            return Ok(None);
        };
        let Some(threads_dir) = self.existing_real_dir(&self.threads_path(), "论坛主题目录")?
        else {
            return Ok(None);
        };
        let author = draft.author.trim();
        let title = draft.title.trim();
        let hash = content_hash(&draft.body);
        for id in self.thread_ids_in(&threads_dir)? {
            let dir = threads_dir.join(&id);
            let Ok(thread) = self.read_thread_in(&dir) else {
                continue;
            };
            if thread.author != author
                || thread.title != title
                || thread.status != ThreadStatus::Open
                || thread.tags != draft.tags
            {
                continue;
            }
            // 读帖失败不判重（尽力而为，允许新建）——不因一条损坏主题 brick 幂等路径。
            let Ok(posts) = self.read_posts_in(&dir) else {
                continue;
            };
            let Some(opening) = posts.first() else {
                continue;
            };
            if opening.origin.as_deref() == Some(origin)
                && same_content(opening, &hash, &draft.body)
            {
                return Ok(Some(thread));
            }
        }
        Ok(None)
    }

    fn summarize(&self, thread_dir: &Path) -> Result<ThreadSummary, String> {
        let thread = self.read_thread_in(thread_dir)?;
        let posts = self.read_posts_in(thread_dir)?;
        // `last_active` 取真实最大 `created`（时钟回拨/手改也不会倒退）；
        // 同毫秒则按 `seq`（写入顺序）、再按 `id` 做确定性兜底，
        // 保证「最后发言」不会因随机 id 而飘。
        let last = posts.iter().max_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| a.seq.cmp(&b.seq))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(ThreadSummary {
            post_count: posts.len(),
            last_active: last.map(|post| post.created).unwrap_or(thread.created),
            last_author: last.map(|post| post.author.clone()),
            thread,
        })
    }
}

impl ForumRepo for FileForumRepo {
    fn create_thread(&self, draft: &NewThread) -> Result<Thread, String> {
        validate_title(&draft.title)?;
        validate_author(draft.author.trim())?;
        for mention in &draft.mentions {
            pipi_protocol::forum::validate_mention(mention)?;
        }
        let _guard = self.lock()?;

        // 幂等：同作者 + 同标题 + 首帖内容哈希相同 ⇒ 视为重放，返回已有主题。
        if let Some(existing) = self.find_duplicate_thread(draft)? {
            return Ok(existing);
        }

        let threads_dir = self.ensured_real_dir(&self.threads_path(), "论坛主题目录")?;
        let id = new_forum_id();
        let thread_dir = threads_dir.join(&id);
        let posts_dir = thread_dir.join("posts");
        fs::create_dir_all(&posts_dir)
            .map_err(|error| format!("无法创建主题目录 {}: {error}", thread_dir.display()))?;

        let thread = Thread {
            id: id.clone(),
            title: draft.title.trim().to_string(),
            author: draft.author.trim().to_string(),
            created: pipi_protocol::now_millis(),
            tags: draft.tags.clone(),
            status: ThreadStatus::Open,
            pinned: false,
        };
        self.write_thread_in(&thread_dir, &thread)?;
        let first_post = NewPost {
            author: draft.author.trim().to_string(),
            body: draft.body.clone(),
            via: draft.via,
            reply_to: None,
            mentions: draft.mentions.clone(),
            origin: draft.origin.clone(),
        };
        // 首帖失败则回收半成品主题目录，避免留下无帖空壳。
        if let Err(error) = self.append_post_in(&thread_dir, &first_post) {
            let _ = fs::remove_dir_all(&thread_dir);
            return Err(error);
        }
        Ok(thread)
    }

    fn thread(&self, thread_id: &str) -> Result<Thread, String> {
        let (dir, _) = self.locate(thread_id)?;
        self.read_thread_in(&dir)
    }

    fn list_threads(
        &self,
        filter: &ThreadFilter,
        limit: usize,
    ) -> Result<Vec<ThreadSummary>, String> {
        let mut candidates: Vec<(String, bool)> = Vec::new();
        if let Some(dir) = self.existing_real_dir(&self.threads_path(), "论坛主题目录")? {
            candidates.extend(self.thread_ids_in(&dir)?.into_iter().map(|id| (id, false)));
        }
        if filter.include_archived
            && let Some(dir) = self.existing_real_dir(&self.archive_path(), "归档目录")?
        {
            candidates.extend(self.thread_ids_in(&dir)?.into_iter().map(|id| (id, true)));
        }
        let mut summaries = Vec::new();
        for (id, archived) in candidates {
            let dir = if archived {
                self.archive_path().join(&id)
            } else {
                self.threads_path().join(&id)
            };
            let Ok(summary) = self.summarize(&dir) else {
                continue;
            };
            if let Some(status) = filter.status
                && summary.thread.status != status
            {
                continue;
            }
            if let Some(tag) = &filter.tag
                && !summary.thread.tags.iter().any(|value| value == tag)
            {
                continue;
            }
            summaries.push(summary);
        }
        // 置顶优先，再按最后活跃倒序；`id` 兜底使顺序确定可复现。
        summaries.sort_by(|a, b| {
            b.thread
                .pinned
                .cmp(&a.thread.pinned)
                .then(b.last_active.cmp(&a.last_active))
                .then(a.thread.id.cmp(&b.thread.id))
        });
        summaries.truncate(limit);
        Ok(summaries)
    }

    fn posts(&self, thread_id: &str, range: PostRange) -> Result<Vec<Post>, String> {
        let (dir, _) = self.locate(thread_id)?;
        let mut posts = self.read_posts_in(&dir)?;
        posts.drain(..range.offset.min(posts.len()));
        if let Some(limit) = range.limit {
            posts.truncate(limit);
        }
        Ok(posts)
    }

    fn append_post(&self, thread_id: &str, draft: &NewPost) -> Result<Post, String> {
        // 先取锁再检查：关帖/归档与回帖构成临界区，消除 TOCTOU。
        let _guard = self.lock()?;
        let (dir, archived) = self.locate(thread_id)?;
        if archived {
            return Err(format!("Thread \"{thread_id}\" is archived, cannot reply"));
        }
        let thread = self.read_thread_in(&dir)?;
        if thread.status == ThreadStatus::Closed {
            return Err(format!("Thread \"{thread_id}\" is closed, cannot reply"));
        }
        self.append_post_in(&dir, draft)
    }

    fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchHit>, String> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Err("Search query cannot be empty".into());
        }
        let needle_chars = needle.chars().count();
        // 活跃区，以及（可选）归档区依次扫描。
        let mut regions: Vec<PathBuf> = Vec::new();
        if let Some(dir) =
            self.existing_real_dir(&self.threads_path(), "forum threads directory")?
        {
            regions.push(dir);
        }
        if options.include_archived
            && let Some(dir) = self.existing_real_dir(&self.archive_path(), "archive directory")?
        {
            regions.push(dir);
        }
        let mut hits = Vec::new();
        'regions: for region in regions {
            for id in self.thread_ids_in(&region)? {
                if hits.len() >= options.limit {
                    break 'regions;
                }
                let dir = region.join(&id);
                let Ok(thread) = self.read_thread_in(&dir) else {
                    continue;
                };
                let Ok(posts) = self.read_posts_in(&dir) else {
                    continue;
                };
                // 正文命中优先，其次标题命中（标题是独立元数据，必须可搜）。
                let body_hit = posts.iter().find_map(|post| {
                    find_case_insensitive(&post.body, &needle).map(|at| (post, at))
                });
                let title_at = find_case_insensitive(&thread.title, &needle);
                if body_hit.is_none() && title_at.is_none() {
                    continue;
                }
                let (post_id, post_seq, author, snippet) = match body_hit {
                    Some((post, at)) => (
                        post.id.clone(),
                        post.seq,
                        post.author.clone(),
                        snippet_around(&post.body, at, needle_chars),
                    ),
                    None => {
                        let first = posts.first();
                        (
                            first.map(|post| post.id.clone()).unwrap_or_default(),
                            first.map(|post| post.seq).unwrap_or(0),
                            first
                                .map(|post| post.author.clone())
                                .unwrap_or_else(|| thread.author.clone()),
                            snippet_around(&thread.title, title_at.unwrap_or(0), needle_chars),
                        )
                    }
                };
                hits.push(SearchHit {
                    thread_id: thread.id.clone(),
                    thread_title: thread.title.clone(),
                    post_id,
                    post_seq,
                    author,
                    snippet,
                });
            }
        }
        Ok(hits)
    }

    fn set_status(&self, thread_id: &str, status: ThreadStatus) -> Result<(), String> {
        let _guard = self.lock()?;
        let (dir, archived) = self.locate(thread_id)?;
        if archived {
            return Err(format!("Thread \"{thread_id}\" is archived"));
        }
        self.patch_thread_in(&dir, |table| {
            table.insert("status".into(), toml::Value::String(status.as_str().into()));
        })
    }

    fn set_pinned(&self, thread_id: &str, pinned: bool) -> Result<(), String> {
        let _guard = self.lock()?;
        let (dir, archived) = self.locate(thread_id)?;
        if archived {
            return Err(format!("Thread \"{thread_id}\" is archived"));
        }
        self.patch_thread_in(&dir, |table| {
            table.insert("pinned".into(), toml::Value::Boolean(pinned));
        })
    }

    fn archive(&self, thread_id: &str) -> Result<(), String> {
        let _guard = self.lock()?;
        let (dir, archived) = self.locate(thread_id)?;
        if archived {
            return Err(format!("Thread \"{thread_id}\" is already archived"));
        }
        let target = self
            .ensured_real_dir(&self.archive_path(), "archive directory")?
            .join(thread_id);
        if self
            .existing_real_dir(&target, "archived thread directory")?
            .is_some()
        {
            return Err(format!("Thread \"{thread_id}\" already exists in archive"));
        }
        fs::rename(&dir, &target).map_err(|error| format!("Failed to archive thread: {error}"))
    }

    fn restore(&self, thread_id: &str) -> Result<(), String> {
        let _guard = self.lock()?;
        let (dir, archived) = self.locate(thread_id)?;
        if !archived {
            return Err(format!("Thread \"{thread_id}\" is not in archive"));
        }
        let target = self
            .ensured_real_dir(&self.threads_path(), "forum threads directory")?
            .join(thread_id);
        if self
            .existing_real_dir(&target, "thread directory")?
            .is_some()
        {
            return Err(format!(
                "Thread \"{thread_id}\" already exists in active region"
            ));
        }
        fs::rename(&dir, &target).map_err(|error| format!("Failed to restore thread: {error}"))
    }
}

/// 主题标题字符上限（写入侧限制，避免超长标题进入摘要 / 提示）。
const MAX_TITLE_CHARS: usize = 300;
/// 作者名字符上限。
const MAX_AUTHOR_CHARS: usize = 100;

/// 标题校验：trim 后非空、不超长、无控制字符与换行（从源头挡住摘要注入）。
fn validate_title(title: &str) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Thread title cannot be empty".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "Thread title too long (maximum {MAX_TITLE_CHARS} characters)"
        ));
    }
    if title.chars().any(char::is_control) {
        return Err("Thread title cannot contain control characters or newlines".into());
    }
    Ok(())
}

/// 作者名校验：trim 后非空、不超长、无控制字符与换行（自报身份也不许破坏行结构）。
fn validate_author(author: &str) -> Result<(), String> {
    let author = author.trim();
    if author.is_empty() {
        return Err("Post author cannot be empty".into());
    }
    if author.chars().count() > MAX_AUTHOR_CHARS {
        return Err(format!(
            "Author name too long (maximum {MAX_AUTHOR_CHARS} characters)"
        ));
    }
    if author.chars().any(char::is_control) {
        return Err("Author name cannot contain control characters or newlines".into());
    }
    Ok(())
}

/// 内容是否相同：优先比 `content_hash`；旧帖（无 hash）退化为逐字节比正文。
fn same_content(post: &Post, hash: &str, body: &str) -> bool {
    match post.content_hash.as_deref() {
        Some(existing) => existing == hash,
        None => post.body == body,
    }
}

/// 内容哈希：正文**归一化行尾后**的 sha256（`sha256:<hex>`），用于幂等去重。
///
/// 只影响去重键；正文在磁盘上仍逐字节保真。
fn content_hash(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    let digest = Sha256::digest(normalized.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    format!("sha256:{hex}")
}

/// 以命中的**字符边界**为锚，截取命中位置附近的片段。
fn snippet_around(text: &str, byte_at: usize, needle_chars: usize) -> String {
    let start_chars = text[..byte_at].chars().count();
    let total_chars = text.chars().count();
    let half = MAX_SNIPPET_CHARS.saturating_sub(needle_chars.min(MAX_SNIPPET_CHARS)) / 2;
    let start = start_chars.saturating_sub(half);
    let end = (start + MAX_SNIPPET_CHARS).min(total_chars);
    let snippet: String = text.chars().skip(start).take(end - start).collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < total_chars { "…" } else { "" };
    format!("{prefix}{}{suffix}", snippet.replace('\n', " "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::forum::PostVia;

    fn repo() -> FileForumRepo {
        let dir = pipi_test_support::temp_dir("pipi-forum-unit");
        FileForumRepo::open(dir).unwrap()
    }

    fn new_thread(author: &str, title: &str, body: &str) -> NewThread {
        NewThread {
            title: title.into(),
            author: author.into(),
            body: body.into(),
            tags: vec![],
            mentions: vec![],
            via: PostVia::Cli,
            origin: Some("test-run".into()),
        }
    }

    fn new_post(author: &str, body: &str, reply_to: Option<&str>) -> NewPost {
        NewPost {
            author: author.into(),
            body: body.into(),
            via: PostVia::Tool,
            reply_to: reply_to.map(str::to_string),
            mentions: vec![],
            origin: None,
        }
    }

    #[test]
    fn init_creates_marker_and_dirs_idempotently() {
        let dir = pipi_test_support::temp_dir("pipi-forum-init");
        let root = FileForumRepo::init(dir.join("board")).unwrap();
        assert!(root.join(crate::root::FORUM_MARKER).is_file());
        assert!(root.join("threads").is_dir());
        assert!(root.join(".archive").is_dir());
        // 幂等：再 init 一次不报错也不破坏已有的主题。
        let repo = FileForumRepo::open(&root).unwrap();
        repo.create_thread(&new_thread("a", "现有的", "正文"))
            .unwrap();
        assert_eq!(FileForumRepo::init(&root).unwrap(), root);
        assert_eq!(
            repo.list_threads(&ThreadFilter::default(), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn creates_lists_and_reads_threads() {
        let repo = repo();
        let thread = repo
            .create_thread(&new_thread("builder", "并发模型", "起帖正文"))
            .unwrap();
        repo.append_post(&thread.id, &new_post("reviewer", "我看看", None))
            .unwrap();

        let list = repo.list_threads(&ThreadFilter::default(), 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].post_count, 2);
        assert_eq!(list[0].last_author.as_deref(), Some("reviewer"));

        let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
        assert_eq!(posts.len(), 2);
        // 首帖是它自己的根、深度 0。
        assert_eq!(posts[0].root_id.as_deref(), Some(posts[0].id.as_str()));
        assert_eq!(posts[0].causality_depth, 0);
    }

    #[test]
    fn reply_resolves_causal_chain_and_rejects_dangling_targets() {
        let repo = repo();
        let thread = repo.create_thread(&new_thread("a", "链", "根帖")).unwrap();
        let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
        let root = posts[0].id.clone();
        let reply = repo
            .append_post(&thread.id, &new_post("b", "回复根帖", Some(&root)))
            .unwrap();
        assert_eq!(reply.causality_depth, 1);
        assert_eq!(reply.root_id.as_deref(), Some(root.as_str()));
        let nested = repo
            .append_post(&thread.id, &new_post("c", "回复回复", Some(&reply.id)))
            .unwrap();
        assert_eq!(nested.causality_depth, 2);
        assert_eq!(nested.root_id.as_deref(), Some(root.as_str()));
        // 指向不存在的帖子：拒绝。
        assert!(
            repo.append_post(&thread.id, &new_post("d", "悬空", Some("does-not-exist")))
                .is_err()
        );
    }

    #[test]
    fn search_finds_title_and_body_hits() {
        let repo = repo();
        let a = repo
            .create_thread(&new_thread("a", "关于沙箱", "正文无关"))
            .unwrap();
        repo.create_thread(&new_thread("b", "别的话题", "沙箱模式有三档"))
            .unwrap();
        // 标题命中 + 正文命中；各主题最多一处。
        let hits = repo.search("沙箱", SearchOptions::default()).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|hit| hit.thread_id == a.id));
        assert!(hits.iter().all(|hit| hit.snippet.contains("沙箱")));
    }

    #[test]
    fn body_mentions_are_merged_into_the_stored_post() {
        let repo = repo();
        let thread = repo
            .create_thread(&new_thread(
                "host",
                "起帖",
                "请 @reviewer 看下，也 @planner",
            ))
            .unwrap();
        let posts = repo.posts(&thread.id, PostRange::default()).unwrap();
        assert_eq!(posts[0].mentions, vec!["reviewer", "planner"]);
        // 显式 mentions 与正文解析合并、去重。
        let reply = repo
            .append_post(
                &thread.id,
                &NewPost {
                    author: "b".into(),
                    body: "@reviewer 补充".into(),
                    via: PostVia::Tool,
                    reply_to: None,
                    mentions: vec!["host".into()],
                    origin: None,
                },
            )
            .unwrap();
        assert_eq!(reply.mentions, vec!["host", "reviewer"]);
    }

    #[test]
    fn search_can_include_archived_threads() {
        let repo = repo();
        let thread = repo
            .create_thread(&new_thread("a", "归档的", "里面有独特词 zebra"))
            .unwrap();
        repo.archive(&thread.id).unwrap();
        // 默认不搜归档。
        assert!(
            repo.search("zebra", SearchOptions::default())
                .unwrap()
                .is_empty()
        );
        // 显式开启后能搜到。
        let hits = repo
            .search(
                "zebra",
                SearchOptions {
                    limit: 10,
                    include_archived: true,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread_id, thread.id);
    }

    #[test]
    fn identical_thread_creation_is_idempotent() {
        let repo = repo();
        let first = repo
            .create_thread(&new_thread("a", "同名", "同正文"))
            .unwrap();
        let again = repo
            .create_thread(&new_thread("a", "同名", "同正文"))
            .unwrap();
        assert_eq!(first.id, again.id, "重放建主题应返回同一主题");
        assert_eq!(
            repo.list_threads(&ThreadFilter::default(), 10)
                .unwrap()
                .len(),
            1
        );
        // 标题或正文不同 → 新建。
        let other = repo
            .create_thread(&new_thread("a", "同名", "别的正文"))
            .unwrap();
        assert_ne!(other.id, first.id);
    }

    #[test]
    fn identical_reply_with_same_origin_is_idempotent() {
        let repo = repo();
        let thread = repo
            .create_thread(&new_thread("a", "标题", "根帖"))
            .unwrap();
        let post = |origin: Option<&str>| NewPost {
            author: "b".into(),
            body: "同一条".into(),
            via: PostVia::Tool,
            reply_to: None,
            mentions: vec![],
            origin: origin.map(str::to_string),
        };
        // 同一原请求（同 origin）重试 ⇒ 返回同一帖。
        let first = repo.append_post(&thread.id, &post(Some("run:1"))).unwrap();
        let again = repo.append_post(&thread.id, &post(Some("run:1"))).unwrap();
        assert_eq!(first.id, again.id, "同 origin 重试应返回同一帖");
        assert!(first.content_hash.is_some());
        // 不同 origin（不同 run）⇒ 新建。
        let run2 = repo.append_post(&thread.id, &post(Some("run:2"))).unwrap();
        assert_ne!(run2.id, first.id, "不同 origin 是不同请求");
        // 人写（无 origin）不去重：合法重复允许。
        let human_a = repo.append_post(&thread.id, &post(None)).unwrap();
        let human_b = repo.append_post(&thread.id, &post(None)).unwrap();
        assert_ne!(human_a.id, human_b.id, "无 origin 的普通写入不去重");
        // 同 origin、同内容但不同父帖 ⇒ 不算重放。
        let mut nested_draft = post(Some("run:1"));
        nested_draft.reply_to = Some(first.id.clone());
        let nested = repo.append_post(&thread.id, &nested_draft).unwrap();
        assert_ne!(nested.id, first.id);
    }

    #[test]
    fn closed_threads_reject_posts_and_archive_moves_them() {
        let repo = repo();
        let thread = repo
            .create_thread(&new_thread("a", "要关掉的", "正文"))
            .unwrap();
        repo.set_status(&thread.id, ThreadStatus::Closed).unwrap();
        assert!(
            repo.append_post(&thread.id, &new_post("b", "能回吗", None))
                .is_err()
        );

        repo.archive(&thread.id).unwrap();
        assert!(repo.thread(&thread.id).is_ok());
        assert!(
            repo.list_threads(&ThreadFilter::default(), 10)
                .unwrap()
                .is_empty()
        );
        let archived = repo
            .list_threads(
                &ThreadFilter {
                    include_archived: true,
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(archived.len(), 1);
        repo.restore(&thread.id).unwrap();
        assert_eq!(
            repo.list_threads(&ThreadFilter::default(), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn invalid_ids_cannot_escape_the_root() {
        let repo = repo();
        assert!(repo.thread("../escape").is_err());
        assert!(repo.posts("nested/name", PostRange::default()).is_err());
    }

    #[test]
    fn empty_body_and_title_are_rejected() {
        let repo = repo();
        assert!(repo.create_thread(&new_thread("a", "  ", "正文")).is_err());
        assert!(repo.create_thread(&new_thread("a", "标题", "   ")).is_err());
    }

    #[test]
    fn thread_metadata_updates_preserve_unknown_fields() {
        let repo = repo();
        let thread = repo
            .create_thread(&new_thread("a", "标题", "正文"))
            .unwrap();
        let path = repo
            .root
            .join("threads")
            .join(&thread.id)
            .join("thread.toml");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("board = \"design\"\n");
        std::fs::write(&path, content).unwrap();

        repo.set_pinned(&thread.id, true).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("board = \"design\""),
            "未知字段被吞: {after}"
        );
        assert!(repo.thread(&thread.id).unwrap().pinned);
    }
}
