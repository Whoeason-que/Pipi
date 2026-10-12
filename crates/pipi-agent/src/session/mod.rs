//! JSONL 载体的实现（写入侧账本与会话仓库）。
//!
//! 会话的**格式与语义**（条目模型、回放规则）与**两个端口**
//! （`SessionLog` / `SessionRepo`）在 `pipi_protocol::session`；本模块只放
//! 今天唯一载体的实现：树状 JSONL 文件，append-only，崩溃安全。
//!
//! 换载体（如 git 分支）时，格式、端口与调用点都不动 —— 只加实现。

use pipi_protocol::session::{EntryKind, SessionEntry};

/// 保留区间在 id 序列里的起点下标。空 id 或定位失败 → `ids.len()`
/// （即全部替换，宁可少保留也不猜位置 —— 猜错会让回放出现无主消息）。
fn kept_start(ids: &[String], keep_from_entry: &str) -> usize {
    if keep_from_entry.is_empty() {
        return ids.len();
    }
    ids.iter()
        .position(|id| id == keep_from_entry)
        .unwrap_or(ids.len())
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

mod history;
mod ledger;
mod repo;

pub use history::load_session;
pub use ledger::SessionWriter;
pub use repo::{JsonlSessionRepo, archive_session_file, fork_session, list_session_summaries};
