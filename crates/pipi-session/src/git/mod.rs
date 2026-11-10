//! git 载体：一个会话 = 一条分支，一条 entry 一个文件，一轮一个 commit。
//!
//! 影子仓库是 Agent 目录下的 bare 仓库 `sessions.git`（绝不碰用户项目的
//! `.git`）：
//!
//! ```text
//! <agent>/sessions.git/
//!   refs/heads/session/<id>   一条会话线（压缩/分叉后是新分支，零复制）
//!   refs/heads/archive/<id>   归档（= JSONL 的 sessions/.archive/）
//!   refs/heads/main           自述锚提交，让裸仓库的 git log 可用
//! ```
//!
//! 完整设计（布局、实测数字、验收口径）见 `docs/git-session-store.md`。
//! 本模块只实现契约层的两个端口；条目格式、回放规则与 id 规则都在
//! `pipi_protocol::session`，调用点不感知载体。
//!
//! 缺系统 git 时 fail-closed：不静默回落成 JSONL（否则同一个 Agent 出现两种
//! 载体，列表/归档语义会混乱），错误文案给出可执行提示。

mod exec;
mod journal;
mod layout;
mod reader;
mod refs;
mod sink;

pub use exec::GitExec;
pub use layout::entry_path;
pub use sink::GitSink;

use std::path::PathBuf;
use std::sync::Arc;

use pipi_protocol::session::{
    SessionCommit, SessionCommitEntry, SessionEntry, SessionLog, SessionRepo, SessionSummary,
    validate_session_id,
};
use pipi_protocol::{new_id, now_millis};

/// 树里展示的最大提交数（长会话只取最近这么多轮，避免一次 git log 太重）。
const HISTORY_MAX_COMMITS: usize = 200;

/// [`SessionRepo`] 的 git 实现：一个 Agent 的 `sessions.git` 影子仓库。
pub struct GitSessionRepo {
    exec: Arc<GitExec>,
}

impl GitSessionRepo {
    /// 以影子仓库目录构造（宿主给的通常是 `<agent>/sessions.git`）。
    pub fn new(repo_dir: PathBuf) -> Self {
        Self {
            exec: Arc::new(GitExec::new(repo_dir)),
        }
    }

    /// 从外部条目序列导入一条会话（`import_sessions` 用）。
    ///
    /// 沿用原会话 id（分支名 = id，id 形态零变更），按「一轮一个 commit」
    /// 切分：每个 user 条目起一个新批次。`archived` 表示来源在归档区 ——
    /// 导入后要落在 `archive/<id>`，别把归档会话搬回活跃区。分支已存在且内容
    /// 一致时返回 `Ok(false)`（幂等）；导入后读回校验，不一致就删掉刚建的分支
    /// 并报错。
    pub fn import(
        &self,
        session_id: &str,
        entries: &[SessionEntry],
        archived: bool,
    ) -> Result<bool, String> {
        validate_session_id(session_id)?;
        if entries.is_empty() {
            return Ok(false);
        }
        let ref_name = refs::session_ref(session_id);
        for existing_ref in [&ref_name, &refs::archive_ref(session_id)] {
            if self.exec.ref_exists(existing_ref)? {
                let existing = self.read_session(existing_ref)?;
                return if same_entries(&existing, entries) {
                    Ok(false)
                } else {
                    Err("Session with the same ID already exists with different content".into())
                };
            }
        }
        self.exec.ensure_ready()?;
        let mut sink = GitSink::from_entries(self.exec.clone(), session_id.to_string(), &[], 0);
        write_batched(&mut sink, entries)?;

        let read_back = self.read_session(&ref_name)?;
        if !same_entries(&read_back, entries) {
            let _ = refs::delete_branch(&self.exec, &ref_name);
            return Err("Import validation failed: read back does not match original".into());
        }
        if archived {
            // 来源在归档区：把刚建的分支移过去（ref 迁移，零复制）
            refs::archive(&self.exec, session_id)?;
        }
        Ok(true)
    }

    fn read_session(&self, ref_name: &str) -> Result<Vec<SessionEntry>, String> {
        match reader::read_entries(&self.exec, ref_name) {
            Ok(entries) => Ok(entries),
            Err(error) => {
                // 区分「分支不存在」与其他失败：ls-tree 报的是同一类 fatal
                if !self.exec.ref_exists(ref_name)? {
                    Err("Session does not exist".into())
                } else {
                    Err(error)
                }
            }
        }
    }
}

impl SessionRepo for GitSessionRepo {
    fn create(&self) -> Result<Box<dyn SessionLog>, String> {
        // 建会话前先确保仓库可用：缺 git 或仓库目录被换成链接都当场报错，
        // 不产生一个「先建出来、提交时才炸」的半成品会话。
        self.exec.ensure_ready()?;
        Ok(Box::new(GitSink::create(self.exec.clone())))
    }

    fn open(&self, session_id: &str) -> Result<Box<dyn SessionLog>, String> {
        self.open_with_entries(session_id).map(|(log, _)| log)
    }

    fn open_with_entries(
        &self,
        session_id: &str,
    ) -> Result<(Box<dyn SessionLog>, Vec<SessionEntry>), String> {
        validate_session_id(session_id)?;
        let ref_name = refs::session_ref(session_id);
        let mut entries = self.read_session(&ref_name)?;
        let turn = reader::count_turns(&self.exec, &ref_name)?;
        let mut sink =
            GitSink::from_entries(self.exec.clone(), session_id.to_string(), &entries, turn);
        let pending = journal::recover(&self.exec, session_id, &entries)?;
        for entry in &pending {
            sink.restore_pending(entry);
        }
        entries.extend(pending);
        sink.load_partial()
            .map_err(|e| format!("Cannot recover partial response: {e}"))?;
        Ok((Box::new(sink), entries))
    }

    fn exists(&self, session_id: &str) -> Result<bool, String> {
        validate_session_id(session_id)?;
        self.exec.ref_exists(&refs::session_ref(session_id))
    }

    fn entries(&self, session_id: &str) -> Result<Vec<SessionEntry>, String> {
        validate_session_id(session_id)?;
        let mut entries = self.read_session(&refs::session_ref(session_id))?;
        let pending = journal::recover(&self.exec, session_id, &entries)?;
        entries.extend(pending);
        Ok(entries)
    }

    fn list(&self) -> Vec<SessionSummary> {
        // 与 JSONL 实现同口径：列表失败不拖垮界面（返回空）
        let mut summaries =
            reader::read_summaries(&self.exec, refs::SESSION_PREFIX).unwrap_or_default();
        for summary in &mut summaries {
            if !journal::has_files(&self.exec, &summary.id).unwrap_or(false) {
                continue;
            }
            let Ok(committed) = self.read_session(&refs::session_ref(&summary.id)) else {
                continue;
            };
            let Ok(pending) = journal::recover(&self.exec, &summary.id, &committed) else {
                continue;
            };
            if pending.is_empty() {
                continue;
            }
            if summary.title == "(空会话)" {
                summary.title = crate::title_from_entries(committed.iter().chain(pending.iter()));
            }
            for entry in &pending {
                if let pipi_protocol::session::EntryKind::Message { message } = &entry.kind {
                    summary.message_count += 1;
                    summary.last_active = summary.last_active.max(message.timestamp());
                }
            }
        }
        summaries
    }

    fn list_archived(&self) -> Result<Vec<SessionSummary>, String> {
        reader::read_summaries(&self.exec, refs::ARCHIVE_PREFIX)
    }

    fn entries_archived(&self, session_id: &str) -> Result<Vec<SessionEntry>, String> {
        validate_session_id(session_id)?;
        self.read_session(&refs::archive_ref(session_id))
    }

    fn history(&self, session_id: &str) -> Result<Vec<SessionCommit>, String> {
        validate_session_id(session_id)?;
        let ref_name = refs::session_ref(session_id);
        if !self.exec.ref_exists(&ref_name)? {
            return Ok(Vec::new());
        }
        let tip = self.exec.resolve_ref(&ref_name)?.unwrap_or_default();
        // 一次 git log 同时拿提交元数据与新增路径：`%x1e` 分隔提交，`%x1f`
        // 分隔字段，`--raw` 的 `:...` 行带出本轮新增的 entry 文件。
        let max = format!("--max-count={HISTORY_MAX_COMMITS}");
        let output = self.exec.run(&[
            "log",
            max.as_str(),
            "--format=%x1e%H%x1f%P%x1f%ct%x1f%B",
            "--raw",
            "--no-renames",
            "--diff-filter=A",
            "--root",
            &ref_name,
        ])?;
        let text = String::from_utf8_lossy(&output);
        let mut commits = Vec::new();
        for record in text.split('\u{1e}') {
            if record.trim().is_empty() {
                continue;
            }
            let mut parts = record.splitn(4, '\u{1f}');
            let id = parts.next().unwrap_or_default().trim().to_string();
            if id.is_empty() {
                continue;
            }
            let parents = parts.next().unwrap_or_default().trim();
            let timestamp = parts
                .next()
                .unwrap_or_default()
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            let rest = parts.next().unwrap_or_default();
            let parent_id = parents.split_whitespace().next().map(str::to_string);
            let mut body_lines: Vec<&str> = Vec::new();
            let mut entries = Vec::new();
            let mut in_raw = false;
            for line in rest.lines() {
                // 只把「raw 行 + entry 路径」当成文件列表；提交正文里偶然出现的
                // `:`/tab 不会提前截断 Turn trailer 的解析。
                let path = line.rsplit('\t').next().unwrap_or_default().trim();
                if line.starts_with(':') && path.starts_with("e/") {
                    in_raw = true;
                    if let Some((seq, kind)) = layout::parse_entry_path(path)
                        && let Some(id8) = layout::entry_id_prefix(path)
                    {
                        entries.push(SessionCommitEntry { id: id8, seq, kind });
                    }
                } else if !in_raw {
                    body_lines.push(line);
                }
            }
            entries.sort_by_key(|entry| entry.seq);
            let summary = body_lines
                .first()
                .map_or("", |line| line.trim())
                .to_string();
            let turn = body_lines.iter().find_map(|line| {
                line.trim()
                    .strip_prefix("Turn:")
                    .and_then(|value| value.trim().parse::<u64>().ok())
            });
            commits.push(SessionCommit {
                is_tip: id == tip,
                id,
                parent_id,
                timestamp: timestamp.saturating_mul(1_000),
                summary,
                turn,
                entries,
            });
        }
        Ok(commits)
    }

    fn fork(
        &self,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<Box<dyn SessionLog>, String> {
        validate_session_id(session_id)?;
        let source_ref = refs::session_ref(session_id);
        if !self.exec.ref_exists(&source_ref)? {
            // 还没有任何提交的空会话：与 JSONL 的空源分叉同义 —— 建一条新线
            return self.create();
        }
        let entries = self.read_session(&source_ref)?;
        let pending = journal::recover(&self.exec, session_id, &entries)?;
        let tip = pending
            .last()
            .or_else(|| entries.last())
            .map(|entry| entry.id.as_str());
        if !pending.is_empty()
            || journal::load_partial(&self.exec, session_id, tip)
                .map_err(|e| e.to_string())?
                .is_some()
        {
            return Err("Save the interrupted session before forking it".into());
        }
        let new_session_id = format!("{}-{}", now_millis(), new_id());

        match up_to_entry_id {
            // 整条线：新分支指向源 tip，零复制（共享全部对象）
            None => {
                let commit = self
                    .exec
                    .resolve_ref(&source_ref)?
                    .ok_or_else(|| "会话不存在".to_string())?;
                let new_ref = refs::session_ref(&new_session_id);
                refs::branch_at(&self.exec, &new_ref, &commit)?;
                // 来源元数据与分支分开记录：整条线分叉不产生新提交，tip 上
                // 只有源会话的提交消息，靠 ref 命名空间才能回答「从哪来」。
                if let Err(error) =
                    refs::write_fork_marker(&self.exec, &new_session_id, session_id, &commit)
                {
                    eprintln!("pipi: failed to record fork lineage for {new_session_id}: {error}");
                }
                let turn = reader::count_turns(&self.exec, &new_ref)?;
                Ok(Box::new(GitSink::from_entries(
                    self.exec.clone(),
                    new_session_id,
                    &entries,
                    turn,
                )))
            }
            // 精确到 entry：O2 的提交是轮粒度，直接指向原提交会把同一轮里
            // 目标之后的条目也带进来；改为「从目标提交的父起，把前缀按轮重写」——
            // 内容精确、历史仍与原线共享（blob 按内容寻址，不重复存储）。
            Some(entry_id) => {
                let target = entries
                    .iter()
                    .find(|entry| entry.id == entry_id)
                    .ok_or_else(|| "分叉点不存在".to_string())?;
                let path = layout::entry_path(target);
                let commit = commit_adding(&self.exec, &source_ref, &path)?;
                let parent = self
                    .exec
                    .run(&["rev-parse", "--verify", "--quiet", &format!("{commit}^")])
                    .ok()
                    .map(|output| String::from_utf8_lossy(&output).trim().to_string())
                    .filter(|sha| !sha.is_empty());
                let prefix = entries
                    .iter()
                    .filter(|entry| entry.seq <= target.seq)
                    .cloned()
                    .collect::<Vec<_>>();
                let mut sink =
                    GitSink::from_entries(self.exec.clone(), new_session_id.clone(), &[], 0);
                sink.set_root_parent(parent);
                write_batched(&mut sink, &prefix)?;
                let new_ref = refs::session_ref(&new_session_id);
                match self.exec.resolve_ref(&new_ref) {
                    Ok(Some(tip)) => {
                        if let Err(error) =
                            refs::write_fork_marker(&self.exec, &new_session_id, session_id, &tip)
                        {
                            eprintln!(
                                "pipi: failed to record fork lineage for {new_session_id}: {error}"
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("pipi: failed to read fork tip for {new_session_id}: {error}")
                    }
                }
                Ok(Box::new(sink))
            }
        }
    }

    fn archive(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let ref_name = refs::session_ref(session_id);
        let committed = self.read_session(&ref_name)?;
        if !journal::recover(&self.exec, session_id, &committed)?.is_empty()
            || journal::load_partial(
                &self.exec,
                session_id,
                committed.last().map(|e| e.id.as_str()),
            )
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err("Save the interrupted session before archiving it".into());
        }
        refs::archive(&self.exec, session_id)
    }

    fn restore(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        refs::restore(&self.exec, session_id)
    }

    fn delete(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let ref_name = refs::session_ref(session_id);
        if !self.exec.ref_exists(&ref_name)? {
            return Err("Session does not exist".into());
        }
        // 先删来源元数据 ref，再删分支：否则 marker 会把分支对象继续保活，
        // 紧随其后的 gc --prune=now 清不掉。
        if let Err(error) = refs::delete_fork_markers(&self.exec, session_id) {
            eprintln!("pipi: failed to remove fork lineage refs for {session_id}: {error}");
        }
        refs::delete_branch(&self.exec, &ref_name)?;
        journal::clear(&self.exec, session_id).map_err(|e| e.to_string())
    }

    fn delete_archived(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let ref_name = refs::archive_ref(session_id);
        if !self.exec.ref_exists(&ref_name)? {
            return Err("Session does not exist in archive".into());
        }
        if let Err(error) = refs::delete_fork_markers(&self.exec, session_id) {
            eprintln!("pipi: failed to remove fork lineage refs for {session_id}: {error}");
        }
        refs::delete_branch(&self.exec, &ref_name)
    }
}

/// 找出引入某个路径的提交（append-only：一个路径只会被一个提交写入）。
fn commit_adding(exec: &GitExec, ref_name: &str, path: &str) -> Result<String, String> {
    let output = exec.run(&["log", "--format=%H", ref_name, "--", path])?;
    String::from_utf8_lossy(&output)
        .lines()
        .last()
        .map(str::to_string)
        .ok_or_else(|| "分叉点不存在".to_string())
}

/// 把一批条目按「一轮一个 commit」写进一条线（导入与 entry 级分叉共用）。
fn write_batched(sink: &mut GitSink, entries: &[SessionEntry]) -> Result<(), String> {
    let mut batch: Vec<&SessionEntry> = Vec::new();
    for entry in entries {
        if layout::is_user_entry(entry) && !batch.is_empty() {
            flush_batch(sink, &batch)?;
            batch.clear();
        }
        batch.push(entry);
    }
    if !batch.is_empty() {
        flush_batch(sink, &batch)?;
    }
    Ok(())
}

/// 把一批条目写进 sink 并提交。
fn flush_batch(sink: &mut GitSink, batch: &[&SessionEntry]) -> Result<(), String> {
    for entry in batch {
        sink.push_entry(entry).map_err(|error| error.to_string())?;
    }
    use pipi_protocol::session::SessionLog;
    sink.commit_point().map_err(|error| error.to_string())
}

/// 两个条目序列是否逐条等价（按各自重新序列化的形式比较）。
fn same_entries(left: &[SessionEntry], right: &[SessionEntry]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(a, b)| serde_json::to_string(a).ok() == serde_json::to_string(b).ok())
}
