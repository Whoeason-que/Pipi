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
    SessionEntry, SessionLog, SessionRepo, SessionSummary, validate_session_id,
};
use pipi_protocol::{new_id, now_millis};

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
        let entries = self.read_session(&ref_name)?;
        let turn = reader::count_turns(&self.exec, &ref_name)?;
        let sink = Box::new(GitSink::from_entries(
            self.exec.clone(),
            session_id.to_string(),
            &entries,
            turn,
        ));
        Ok((sink, entries))
    }

    fn exists(&self, session_id: &str) -> Result<bool, String> {
        validate_session_id(session_id)?;
        self.exec.ref_exists(&refs::session_ref(session_id))
    }

    fn entries(&self, session_id: &str) -> Result<Vec<SessionEntry>, String> {
        validate_session_id(session_id)?;
        self.read_session(&refs::session_ref(session_id))
    }

    fn list(&self) -> Vec<SessionSummary> {
        // 与 JSONL 实现同口径：列表失败不拖垮界面（返回空）
        reader::read_summaries(&self.exec, refs::SESSION_PREFIX).unwrap_or_default()
    }

    fn list_archived(&self) -> Result<Vec<SessionSummary>, String> {
        reader::read_summaries(&self.exec, refs::ARCHIVE_PREFIX)
    }

    fn entries_archived(&self, session_id: &str) -> Result<Vec<SessionEntry>, String> {
        validate_session_id(session_id)?;
        self.read_session(&refs::archive_ref(session_id))
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
                let mut sink = GitSink::from_entries(self.exec.clone(), new_session_id, &[], 0);
                sink.set_root_parent(parent);
                write_batched(&mut sink, &prefix)?;
                Ok(Box::new(sink))
            }
        }
    }

    fn archive(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
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
        refs::delete_branch(&self.exec, &ref_name)
    }

    fn delete_archived(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let ref_name = refs::archive_ref(session_id);
        if !self.exec.ref_exists(&ref_name)? {
            return Err("Session does not exist in archive".into());
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
        sink.push_entry(entry);
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
