//! git 载体写侧：完整条目先原子写入恢复文件，轮末通过 `fast-import` 合并为一次提交。
//! 提交失败保留恢复文件，下一轮开始前重试；内存账本仍只用于临时会话。

use std::sync::Arc;

use pipi_protocol::session::{
    CompactionRecord, ContextEditRecord, EntryKind, EnvDeclared, SessionEntry, SessionLog,
};
use pipi_protocol::{BackgroundTaskResult, Message, Model, new_id, now_millis};

use super::exec::GitExec;
use super::{journal, layout, refs};
use crate::LedgerState;

/// 一个 git 分支上的会话账本。
pub struct GitSink {
    session_id: String,
    exec: Arc<GitExec>,
    /// tip / seq / 环境记账 / 消息 id 映射（与 JSONL 载体同一套规则）。
    ledger: LedgerState,
    /// 上一次压缩追加前的账本快照（压缩提交失败时回滚用）。
    /// 只压缩条目需要回滚；普通消息追加不克隆整份账本。
    last_append_ledger: Option<LedgerState>,
    /// 尚未提交的条目（提交点一次 fast-import 落盘）。
    buffer: Vec<SessionEntry>,
    /// 已提交的轮数（`Turn:` trailer 的计数，按提交点递增，不按 user 条目数）。
    turn: u64,
    /// 分支还不存在时首个提交的父提交（entry 级分叉用：从目标提交的父起写前缀，
    /// 既精确到 entry、又与原线共享历史）。
    root_parent: Option<String>,
    recovered: bool,
    partial: Option<Message>,
    uncertain_commit: bool,
}

impl GitSink {
    /// 新建会话：分配 id，分支要到第一次提交才出现。
    pub fn create(exec: Arc<GitExec>) -> GitSink {
        GitSink {
            session_id: format!("{}-{}", now_millis(), new_id()),
            exec,
            ledger: LedgerState::empty(),
            last_append_ledger: None,
            buffer: Vec::new(),
            turn: 0,
            root_parent: None,
            recovered: false,
            partial: None,
            uncertain_commit: false,
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
        GitSink {
            session_id,
            exec,
            ledger: LedgerState::from_entries(entries),
            last_append_ledger: None,
            buffer: Vec::new(),
            turn,
            root_parent: None,
            recovered: false,
            partial: None,
            uncertain_commit: false,
        }
    }

    fn append_entry(&mut self, kind: EntryKind, timestamp: u64) -> std::io::Result<String> {
        let entry = self.ledger.begin(kind, timestamp);
        let id = entry.id.clone();
        self.push_owned(entry)?;
        Ok(id)
    }

    pub fn set_root_parent(&mut self, parent: Option<String>) {
        self.root_parent = parent;
    }

    pub fn push_entry(&mut self, entry: &SessionEntry) -> std::io::Result<()> {
        self.push_owned(entry.clone())
    }

    /// 条目入缓冲后直接写恢复文件（不再克隆整个缓冲）；写失败就弹出，
    /// 缓冲与账本保持与写之前一致。
    fn push_owned(&mut self, entry: SessionEntry) -> std::io::Result<()> {
        self.buffer.push(entry);
        let index = self.buffer.len() - 1;
        if let Err(error) = journal::write_entries(&self.exec, &self.session_id, &self.buffer) {
            self.buffer.pop();
            return Err(error);
        }
        self.ledger.advance(&self.buffer[index]);
        Ok(())
    }

    pub(super) fn restore_pending(&mut self, entry: &SessionEntry) {
        self.ledger.advance(entry);
        self.buffer.push(entry.clone());
        self.recovered = true;
    }

    pub(super) fn load_partial(&mut self) -> std::io::Result<()> {
        self.partial =
            journal::load_partial(&self.exec, &self.session_id, self.ledger.tip_id.as_deref())?;
        self.recovered |= self.partial.is_some();
        Ok(())
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
    fn append_approval(
        &mut self,
        audit: &pipi_protocol::permissions::ApprovalAudit,
    ) -> std::io::Result<String> {
        self.append_entry(
            EntryKind::Approval {
                audit: audit.clone(),
            },
            now_millis(),
        )
    }
    fn append_question(
        &mut self,
        event: &pipi_protocol::interaction::QuestionEvent,
    ) -> std::io::Result<String> {
        self.append_entry(
            EntryKind::Question {
                event: event.clone(),
            },
            now_millis(),
        )
    }
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn is_temporary(&self) -> bool {
        false
    }

    fn tip_id(&self) -> Option<&str> {
        self.ledger.tip_id.as_deref()
    }

    fn message_ids(&self) -> &[String] {
        &self.ledger.message_ids
    }

    fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        let timestamp = message.timestamp();
        self.append_entry(
            EntryKind::Message {
                message: message.clone(),
            },
            timestamp,
        )
    }

    fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String> {
        self.append_entry(
            EntryKind::Custom {
                custom_type: custom_type.to_string(),
            },
            now_millis(),
        )
    }

    fn append_subagent_run(
        &mut self,
        parent_agent: &str,
        parent_session: &str,
    ) -> std::io::Result<String> {
        self.append_entry(
            EntryKind::SubagentRun {
                parent_agent: parent_agent.to_string(),
                parent_session: parent_session.to_string(),
            },
            now_millis(),
        )
    }

    fn append_background_result(
        &mut self,
        result: &BackgroundTaskResult,
    ) -> std::io::Result<String> {
        self.append_entry(
            EntryKind::BackgroundResult {
                result: result.clone(),
            },
            now_millis(),
        )
    }

    fn append_compaction(&mut self, record: &CompactionRecord<'_>) -> std::io::Result<String> {
        // 只有压缩条目需要回滚；快照放在这里，普通消息/工具结果的热路径
        // 不承担整份账本克隆。
        let previous = self.ledger.clone();
        let result = self.append_entry(record.to_entry_kind(), now_millis());
        if result.is_ok() {
            self.last_append_ledger = Some(previous);
        }
        result
    }

    fn append_context_edit(&mut self, record: &ContextEditRecord<'_>) -> std::io::Result<String> {
        // 与 append_compaction 同一套提交失败回滚语义。
        let previous = self.ledger.clone();
        let result = self.append_entry(record.to_entry_kind(), now_millis());
        if result.is_ok() {
            self.last_append_ledger = Some(previous);
        }
        result
    }

    fn append_model_change(&mut self, model: &Model) -> std::io::Result<String> {
        self.append_entry(
            EntryKind::ModelChange {
                provider: model.clone(),
            },
            now_millis(),
        )
    }

    fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>> {
        if self.ledger.env_recorded {
            return Ok(None);
        }
        let id = self.append_entry(EntryKind::Env { declared }, now_millis())?;
        self.ledger.env_recorded = true;
        Ok(Some(id))
    }

    fn has_recovery(&self) -> bool {
        self.recovered
    }

    fn recovered_partial(&self) -> Option<Message> {
        self.partial.clone()
    }

    fn checkpoint_partial(&mut self, message: &Message) -> std::io::Result<()> {
        journal::write_partial(
            &self.exec,
            &self.session_id,
            self.ledger.tip_id.as_deref(),
            message,
        )
    }

    fn clear_partial(&mut self) -> std::io::Result<()> {
        journal::clear_partial(&self.exec, &self.session_id)?;
        self.partial = None;
        Ok(())
    }

    fn commit_point(&mut self) -> std::io::Result<()> {
        let ref_name = refs::session_ref(&self.session_id);
        if self.uncertain_commit || self.recovered {
            let committed = if self.exec.ref_exists(&ref_name).map_err(io_error)? {
                super::reader::read_entries(&self.exec, &ref_name).map_err(io_error)?
            } else {
                Vec::new()
            };
            let pending =
                journal::recover(&self.exec, &self.session_id, &committed).map_err(io_error)?;
            self.ledger = LedgerState::from_entries(&committed);
            self.buffer.clear();
            for entry in pending {
                self.ledger.advance(&entry);
                self.buffer.push(entry);
            }
            self.turn = if self.exec.ref_exists(&ref_name).map_err(io_error)? {
                super::reader::count_turns(&self.exec, &ref_name).map_err(io_error)?
            } else {
                0
            };
            self.uncertain_commit = false;
        }
        // 父提交 = 分支当前 tip；分支还不存在时用 root_parent（entry 级分叉
        // 从目标提交的父起写前缀），都没有就是一条新线的首个提交。
        let parent = match self.exec.resolve_ref(&ref_name).map_err(io_error)? {
            Some(tip) => Some(tip),
            None => self.root_parent.clone(),
        };
        // 空缓冲 + 分支已存在 = 无事可做；空缓冲 + 分支不存在 = 空会话的首个
        // 提交（空会话也要在列表里可见，与 JSONL 的空文件同义）。
        if self.buffer.is_empty() && parent.is_some() {
            self.recovered = false;
            journal::clear(&self.exec, &self.session_id)?;
            return Ok(());
        }
        let has_user = self.buffer.iter().any(layout::is_user_entry);
        let turn = if has_user { Some(self.turn + 1) } else { None };
        let message = layout::commit_message(&self.buffer, turn);
        let stream = self.build_stream(parent.as_deref(), &message);
        if let Err(error) = self.exec.fast_import(stream) {
            self.uncertain_commit = true;
            self.recovered = true;
            return Err(io_error(error));
        }
        self.buffer.clear();
        self.recovered = false;
        self.partial = None;
        if let Err(error) = journal::clear(&self.exec, &self.session_id) {
            eprintln!("pipi: Cannot remove committed recovery files: {error}");
        }
        self.last_append_ledger = None;
        if has_user {
            self.turn += 1;
        }
        // 提交之后再跑节流 gc（自己取锁，失败忽略）
        self.exec.gc_auto();
        Ok(())
    }

    fn rollback_last_entry(&mut self) -> std::io::Result<bool> {
        let Some(previous) = self.last_append_ledger.clone() else {
            return Ok(false);
        };
        if self.buffer.is_empty() {
            return Ok(false);
        }
        journal::write_entries(
            &self.exec,
            &self.session_id,
            &self.buffer[..self.buffer.len() - 1],
        )?;
        self.buffer.pop();
        self.last_append_ledger = None;
        self.ledger = previous;
        Ok(true)
    }
}

fn io_error(message: String) -> std::io::Error {
    std::io::Error::other(message)
}
