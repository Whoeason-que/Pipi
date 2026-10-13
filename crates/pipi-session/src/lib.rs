//! 会话载体：一个 Agent 的会话存储实现。
//!
//! 会话的**格式与语义**（条目模型、回放规则）与**两个端口**
//! （`SessionLog` / `SessionRepo`）在 `pipi_protocol::session`；这里只放实现：
//!
//! - [`GitSessionRepo`] / [`GitSink`]：git 分支，一轮一个 commit —— **唯一**的
//!   持久载体（见 `docs/git-session-store.md`）；
//! - [`MemoryLog`]：设置工作台临时测试会话的内存账本（不落盘）；
//! - [`import_legacy_sessions`] / [`load_session`]：旧 `sessions/*.jsonl` 的显式
//!   导入与逐行读取（逃生通道，供未迁移的机器使用）。

use pipi_protocol::session::{EntryKind, SessionEntry, active_path, kept_start, replay};
use pipi_protocol::{Message, new_id};

/// 会话标题：首个用户消息的首行（截断 48 字符）；没有用户消息时「(空会话)」。
///
/// 列表（git 载体）与内存账本的诊断输出共用这一条规则。
fn title_from_entries<'a>(entries: impl IntoIterator<Item = &'a SessionEntry>) -> String {
    entries
        .into_iter()
        .find_map(|entry| match &entry.kind {
            EntryKind::Message {
                message: Message::User { content, .. },
            } => Some(content.as_str()),
            _ => None,
        })
        .map(title_of)
        .unwrap_or_else(|| "(空会话)".to_string())
}

fn title_of(text: &str) -> String {
    const TITLE_CHARS: usize = 48;
    let cut = truncate_chars(text.lines().next().unwrap_or("").trim(), TITLE_CHARS);
    if cut.is_empty() {
        "(空会话)".to_string()
    } else {
        cut
    }
}

/// 列表里的模型名回退：最近一条带非空模型名的 assistant 消息
/// （没有 model_change 条目时的口径）。
fn latest_assistant_model<'a>(
    entries: impl DoubleEndedIterator<Item = &'a SessionEntry>,
) -> Option<String> {
    entries.rev().find_map(|entry| match &entry.kind {
        EntryKind::Message {
            message: Message::Assistant { model, .. },
        } if !model.is_empty() => Some(model.clone()),
        _ => None,
    })
}

/// 按字符数截断，超出时补省略号（标题与 commit 摘要共用）。
fn truncate_chars(text: &str, max: usize) -> String {
    let mut cut = text.chars().take(max).collect::<String>();
    if text.chars().count() > max {
        cut.push('…');
    }
    cut
}

/// 写侧账本状态：tip / seq / 环境记账 / 消息 id 映射。
///
/// [`GitSink`]（git）与 [`MemoryLog`]（内存）共用这一套推进规则 —— 回放、压缩
/// 的 id 换算与事件语义都依赖它。
struct LedgerState {
    tip_id: Option<String>,
    seq: u64,
    /// 环境记账每会话至多一条；打开旧会话时从已有条目恢复。
    env_recorded: bool,
    /// 与内存消息序列一一对应的条目 id
    /// （规则见 [`pipi_protocol::session::kept_start`]）。
    message_ids: Vec<String>,
}

impl LedgerState {
    fn empty() -> Self {
        Self {
            tip_id: None,
            seq: 0,
            env_recorded: false,
            message_ids: Vec::new(),
        }
    }

    /// 从已读回的条目恢复（`GitSessionRepo::open` 与 `fork` 共用）。
    fn from_entries(entries: &[SessionEntry]) -> Self {
        let (tip_id, seq) = match entries.last() {
            Some(last) => (Some(last.id.clone()), last.seq + 1),
            None => (None, 0),
        };
        let env_recorded = entries
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::Env { .. }));
        // 消息 ↔ 条目 id 映射：与 rebuild_messages 同一套回放规则，
        // 保证「内存第 i 条消息」的条目 id 可查。
        let message_ids = replay(&active_path(entries)).1;
        Self {
            tip_id,
            seq,
            env_recorded,
            message_ids,
        }
    }

    /// 构造挂在当前 tip 上的新条目；持久化成功后由写方调 [`Self::advance`]。
    fn begin(&self, kind: EntryKind, timestamp: u64) -> SessionEntry {
        SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp,
            kind,
        }
    }

    /// 条目入账：推进 tip 与 seq，维护消息 ↔ 条目 id 映射。
    fn advance(&mut self, entry: &SessionEntry) {
        self.tip_id = Some(entry.id.clone());
        self.seq = entry.seq + 1;
        track_message_ids(&mut self.message_ids, entry);
    }
}

/// 压缩条目对 id 序列的作用：摘要消息占用条目自身的 id 槽位，其后跟上
/// 保留区间的 id（规则与 `pipi_protocol::session::replay` 必须一致）。
fn track_message_ids(ids: &mut Vec<String>, entry: &SessionEntry) {
    match &entry.kind {
        EntryKind::Message { .. } => ids.push(entry.id.clone()),
        EntryKind::Compaction {
            keep_from_entry, ..
        } => {
            let kept = ids.split_off(kept_start(ids, keep_from_entry));
            ids.clear();
            ids.push(entry.id.clone());
            ids.extend(kept);
        }
        _ => {}
    }
}

mod git;
mod legacy;
mod memory;

pub use git::{GitExec, GitSessionRepo, GitSink, entry_path};
pub use legacy::{import_legacy_sessions, load_session};
pub use memory::MemoryLog;

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::session::CompactionRecord;

    /// 账本推进（`message_ids`）必须与回放结果一致 —— 下一次压缩的
    /// `keep_from_entry` 换算就靠这份一致性；git 写侧与内存账本共用它。
    #[test]
    fn ledger_message_ids_match_replay() {
        fn append(
            ledger: &mut LedgerState,
            entries: &mut Vec<SessionEntry>,
            kind: EntryKind,
        ) -> String {
            let entry = ledger.begin(kind, 0);
            ledger.advance(&entry);
            let id = entry.id.clone();
            entries.push(entry);
            id
        }
        let mut ledger = LedgerState::empty();
        let mut entries = Vec::new();
        let user = |ledger: &mut LedgerState, entries: &mut Vec<SessionEntry>, text: &str| {
            append(
                ledger,
                entries,
                EntryKind::Message {
                    message: Message::user_text(text),
                },
            )
        };
        user(&mut ledger, &mut entries, "turn 1");
        user(&mut ledger, &mut entries, "turn 2");
        let kept = user(&mut ledger, &mut entries, "turn 3");
        user(&mut ledger, &mut entries, "turn 4");
        let source_tip = ledger.tip_id.clone().unwrap();
        let record = CompactionRecord {
            summary: "前两轮摘要",
            strategy: "llm-summarize",
            keep_from_entry: &kept,
            source_tip: &source_tip,
            usage: None,
        };
        append(&mut ledger, &mut entries, record.to_entry_kind());
        user(&mut ledger, &mut entries, "turn 5");

        let (messages, ids) = replay(&active_path(&entries));
        // 摘要 + 保留的 turn 3/4 + 压缩后的 turn 5
        assert_eq!(messages.len(), 4);
        assert_eq!(ledger.message_ids, ids);
    }
}
