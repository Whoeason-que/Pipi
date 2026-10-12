//! 会话载体：一个 Agent 的会话存储实现。
//!
//! 会话的**格式与语义**（条目模型、回放规则）与**两个端口**
//! （`SessionLog` / `SessionRepo`）在 `pipi_protocol::session`；这里只放实现：
//!
//! - [`SessionWriter`] / [`JsonlSessionRepo`]：树状 JSONL 文件，append-only，
//!   崩溃安全（今天的唯一载体）；
//! - 第二个载体（git 分支，见 `docs/git-session-store.md`）也落在这个 crate ——
//!   格式、端口与调用点都不动，宿主侧的装配点（`runtime/store.rs`）选实现。

use pipi_protocol::session::{EntryKind, SessionEntry, kept_start};

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
