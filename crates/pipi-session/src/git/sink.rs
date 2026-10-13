//! git 载体的写侧：条目先进内存缓冲，提交点用一次 `fast-import` 落盘。
//!
//! [`GitSink`] 是 [`SessionLog`] 的 git 实现。与 JSONL 的 [`SessionWriter`]
//! 相比，唯一的行为差别是**持久化时机**：append 只进缓冲（O2：一轮一个
//! commit），由运行时在提交点调用 `commit_point` 固化 —— 提交失败时缓冲
//! 原样保留（内存状态不前进），下一次提交会把它一起带上。
//!
//! [`SessionWriter`]: crate::SessionWriter

use std::sync::Arc;

use pipi_protocol::session::{
    CompactionRecord, EntryKind, EnvDeclared, SessionEntry, SessionLog, active_path, replay,
};
use pipi_protocol::{Message, Model, new_id, now_millis};

use super::exec::GitExec;
use super::{layout, refs};
use crate::track_message_ids;

/// 一个 git 分支上的会话账本。
pub struct GitSink {
    session_id: String,
    exec: Arc<GitExec>,
    tip_id: Option<String>,
    seq: u64,
    env_recorded: bool,
    message_ids: Vec<String>,
    /// 尚未提交的条目（提交点一次 fast-import 落盘）。
    buffer: Vec<SessionEntry>,
    /// 已提交的轮数（`Turn:` trailer 的计数，按提交点递增，不按 user 条目数）。
    turn: u64,
    /// 分支还不存在时首个提交的父提交（entry 级分叉用：从目标提交的父起写前缀，
    /// 既精确到 entry、又与原线共享历史）。
    root_parent: Option<String>,
}

impl GitSink {
    /// 新建会话：分配 id，分支要到第一次提交才出现。
    pub fn create(exec: Arc<GitExec>) -> GitSink {
        GitSink {
            session_id: format!("{}-{}", now_millis(), new_id()),
            exec,
            tip_id: None,
            seq: 0,
            env_recorded: false,
            message_ids: Vec::new(),
            buffer: Vec::new(),
            turn: 0,
            root_parent: None,
        }
    }

    /// 从已读回的条目恢复账本状态（打开已有会话、分叉、导入共用）。
    ///
    /// `turn` 是该分支上已有的轮数（由 [`super::reader::count_turns`] 数出来），
    /// 保证重开后的轮号继续递增。
    pub fn from_entries(
        exec: Arc<GitExec>,
        session_id: String,
        entries: &[SessionEntry],
        turn: u64,
    ) -> GitSink {
        let (tip_id, seq) = match entries.last() {
            Some(last) => (Some(last.id.clone()), last.seq + 1),
            None => (None, 0),
        };
        let env_recorded = entries
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::Env { .. }));
        // 消息 ↔ 条目 id 映射：与 rebuild_messages 同一套回放规则
        let message_ids = replay(&active_path(entries)).1;
        GitSink {
            session_id,
            exec,
            tip_id,
            seq,
            env_recorded,
            message_ids,
            buffer: Vec::new(),
            turn,
            root_parent: None,
        }
    }

    /// 追加一条 entry：挂在当前 tip 上，只进缓冲（不做 IO）。
    fn append_entry(&mut self, kind: EntryKind, timestamp: u64) -> String {
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp,
            kind,
        };
        let id = entry.id.clone();
        self.push(entry);
        id
    }

    /// 指定分支首个提交的父提交（entry 级分叉用；必须在首次 commit_point 之前调用）。
    pub fn set_root_parent(&mut self, parent: Option<String>) {
        self.root_parent = parent;
    }

    /// 直接写入一条既有 entry（导入旧会话用）：保留原 id/seq/parentId/timestamp。
    pub fn push_entry(&mut self, entry: &SessionEntry) {
        self.push(entry.clone());
    }

    fn push(&mut self, entry: SessionEntry) {
        self.tip_id = Some(entry.id.clone());
        self.seq = entry.seq + 1;
        track_message_ids(&mut self.message_ids, &entry);
        self.buffer.push(entry);
    }

    /// 会话身份（与 [`SessionLog::session_id`] 同源；固有方法便于导入等内部调用）。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 组装一次 `fast-import`：缓冲里的每条 entry 一个 blob，整批一个 commit。
    ///
    /// 空缓冲也是合法的（空会话的首个提交）：此时没有 M 行，提交的是一棵空树。
    fn build_stream(&self, parent: Option<&str>, message: &str) -> Vec<u8> {
        let (role, timestamp) = match self.buffer.first() {
            Some(first) => (layout::entry_role(first), first.timestamp / 1000),
            None => ("pipi", now_millis() / 1000),
        };
        let mut stream = String::new();
        stream.push_str(&format!("commit {}\n", refs::session_ref(&self.session_id)));
        stream.push_str(&format!(
            "author {role} <{role}@pipi.local> {timestamp} +0000\n"
        ));
        stream.push_str(&format!("committer Pipi <pipi@local> {timestamp} +0000\n"));
        stream.push_str(&format!("data {}\n", message.len()));
        stream.push_str(message);
        if let Some(parent) = parent {
            stream.push_str(&format!("from {parent}\n"));
        }
        for entry in &self.buffer {
            // 与 JSONL 逐字节一致：内容就是今天落盘的那一行（含换行）
            let mut line = serde_json::to_string(entry).expect("会话条目可序列化");
            line.push('\n');
            stream.push_str(&format!("M 100644 inline {}\n", layout::entry_path(entry)));
            stream.push_str(&format!("data {}\n", line.len()));
            stream.push_str(&line);
        }
        stream.push_str("done\n");
        stream.into_bytes()
    }
}

impl SessionLog for GitSink {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn is_temporary(&self) -> bool {
        false
    }

    fn tip_id(&self) -> Option<&str> {
        self.tip_id.as_deref()
    }

    fn message_ids(&self) -> &[String] {
        &self.message_ids
    }

    fn mark_env_recorded(&mut self) {
        self.env_recorded = true;
    }

    fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        let timestamp = message.timestamp();
        Ok(self.append_entry(
            EntryKind::Message {
                message: message.clone(),
            },
            timestamp,
        ))
    }

    fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String> {
        Ok(self.append_entry(
            EntryKind::Custom {
                custom_type: custom_type.to_string(),
            },
            now_millis(),
        ))
    }

    fn append_compaction(&mut self, record: &CompactionRecord<'_>) -> std::io::Result<String> {
        Ok(self.append_entry(
            EntryKind::Compaction {
                summary: record.summary.to_string(),
                keep_from_entry: record.keep_from_entry.to_string(),
                source_tip: record.source_tip.to_string(),
                strategy: record.strategy.to_string(),
                usage: record.usage,
            },
            now_millis(),
        ))
    }

    fn append_model_change(&mut self, model: &Model) -> std::io::Result<String> {
        Ok(self.append_entry(
            EntryKind::ModelChange {
                provider: model.clone(),
            },
            now_millis(),
        ))
    }

    fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>> {
        if self.env_recorded {
            return Ok(None);
        }
        let id = self.append_entry(EntryKind::Env { declared }, now_millis());
        self.env_recorded = true;
        Ok(Some(id))
    }

    fn commit_point(&mut self) -> std::io::Result<()> {
        let ref_name = refs::session_ref(&self.session_id);
        // 父提交 = 分支当前 tip；分支还不存在时用 root_parent（entry 级分叉
        // 从目标提交的父起写前缀），都没有就是一条新线的首个提交。
        let parent = match self.exec.resolve_ref(&ref_name).map_err(io_error)? {
            Some(tip) => Some(tip),
            None => self.root_parent.clone(),
        };
        // 空缓冲 + 分支已存在 = 无事可做；空缓冲 + 分支不存在 = 空会话的首个
        // 提交（空会话也要在列表里可见，与 JSONL 的空文件同义）。
        if self.buffer.is_empty() && parent.is_some() {
            return Ok(());
        }
        let has_user = self.buffer.iter().any(layout::is_user_entry);
        let turn = if has_user { Some(self.turn + 1) } else { None };
        let message = layout::commit_message(&self.buffer, turn);
        let stream = self.build_stream(parent.as_deref(), &message);
        self.exec.fast_import(stream).map_err(io_error)?;
        self.buffer.clear();
        if has_user {
            self.turn += 1;
        }
        // 提交之后再跑节流 gc（自己取锁，失败忽略）
        self.exec.gc_auto();
        Ok(())
    }
}

fn io_error(message: String) -> std::io::Error {
    std::io::Error::other(message)
}
