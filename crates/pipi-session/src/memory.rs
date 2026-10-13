//! 内存账本：设置工作台的临时测试会话（不落盘、不进会话列表）。
//!
//! [`MemoryLog`] 是 [`SessionLog`](pipi_protocol::session::SessionLog) 的内存实现：
//! 只维护与持久载体同一套账本状态（[`LedgerState`]：tip / seq / 环境记账 /
//! 消息 id 映射），`commit_point` 是空操作 —— 条目只存在于进程内存，应用退出
//! 即消失。这样 runtime 的事件、统计与压缩不需要两套状态机，同时保证临时测试
//! 不会出现在会话目录和会话列表里。

use pipi_protocol::session::{CompactionRecord, EntryKind, EnvDeclared, SessionLog};
use pipi_protocol::{Message, Model, new_id, now_millis};

use super::LedgerState;

/// 临时测试会话的内存账本。
pub struct MemoryLog {
    session_id: String,
    ledger: LedgerState,
}

impl MemoryLog {
    /// 新建一条临时会话。ID 仍是稳定的会话身份，供事件过滤、停止、steering
    /// 与供应商缓存路由使用。
    pub fn new() -> Self {
        Self {
            session_id: format!("test-{}", new_id()),
            ledger: LedgerState::empty(),
        }
    }

    /// 构造挂在当前 tip 上的新条目并推进账本（持久化载体没有的写侧动作）。
    fn append(&mut self, kind: EntryKind, timestamp: u64) -> String {
        let entry = self.ledger.begin(kind, timestamp);
        self.ledger.advance(&entry);
        entry.id
    }
}

impl Default for MemoryLog {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionLog for MemoryLog {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn is_temporary(&self) -> bool {
        true
    }

    fn tip_id(&self) -> Option<&str> {
        self.ledger.tip_id.as_deref()
    }

    fn message_ids(&self) -> &[String] {
        &self.ledger.message_ids
    }

    fn mark_env_recorded(&mut self) {
        self.ledger.env_recorded = true;
    }

    fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        let timestamp = message.timestamp();
        Ok(self.append(
            EntryKind::Message {
                message: message.clone(),
            },
            timestamp,
        ))
    }

    fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String> {
        Ok(self.append(
            EntryKind::Custom {
                custom_type: custom_type.to_string(),
            },
            now_millis(),
        ))
    }

    fn append_compaction(&mut self, record: &CompactionRecord<'_>) -> std::io::Result<String> {
        Ok(self.append(record.to_entry_kind(), now_millis()))
    }

    fn append_model_change(&mut self, model: &Model) -> std::io::Result<String> {
        Ok(self.append(
            EntryKind::ModelChange {
                provider: model.clone(),
            },
            now_millis(),
        ))
    }

    fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>> {
        if self.ledger.env_recorded {
            return Ok(None);
        }
        let id = self.append(EntryKind::Env { declared }, now_millis());
        self.ledger.env_recorded = true;
        Ok(Some(id))
    }

    fn commit_point(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_keeps_ledger_state_without_a_file() {
        let mut log = MemoryLog::new();
        let session_id = log.session_id().to_string();
        assert!(session_id.starts_with("test-"));
        assert!(log.is_temporary());
        assert!(log.tip_id().is_none());

        let first = log
            .append_message(&Message::user_text("只在内存里"))
            .unwrap();
        let second = log
            .append_message(&Message::assistant_text("收到", "mock"))
            .unwrap();

        assert_eq!(log.tip_id(), Some(second.as_str()));
        assert_eq!(log.message_ids(), [first.as_str(), second.as_str()]);
        // 提交点对内存账本是空操作，不改变任何状态
        log.commit_point().unwrap();
        assert_eq!(log.tip_id(), Some(second.as_str()));
    }

    #[test]
    fn env_accounting_is_idempotent_and_compaction_tracks_ids() {
        let mut log = MemoryLog::new();
        log.append_message(&Message::user_text("turn 1")).unwrap();
        let kept = log.append_message(&Message::user_text("turn 2")).unwrap();
        let declared = vec![EnvDeclared {
            key: "AV_AGENT".into(),
            source: "runtime".into(),
        }];
        assert!(log.append_env(declared.clone()).unwrap().is_some());
        // 幂等：同一会话重复记账不追加
        assert!(log.append_env(declared).unwrap().is_none());

        let tip = log.tip_id().unwrap().to_string();
        let summary = log
            .append_compaction(&CompactionRecord {
                summary: "前一轮摘要",
                strategy: "llm-summarize",
                keep_from_entry: &kept,
                source_tip: &tip,
                usage: None,
            })
            .unwrap();
        let last = log.append_message(&Message::user_text("turn 3")).unwrap();

        // 摘要消息占用压缩条目自身的 id 槽位，保留区间的 id 跟在后面
        assert_eq!(log.message_ids(), [summary, kept, last]);
    }
}
