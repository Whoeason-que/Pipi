//! 会话载体：一个 Agent 的会话存储实现。
//!
//! 会话的**格式与语义**（条目模型、回放规则）与**两个端口**
//! （`SessionLog` / `SessionRepo`）在 `pipi_protocol::session`；这里只放实现：
//!
//! - [`GitSessionRepo`] / [`GitSink`]：git 分支，一轮一个 commit（默认载体，
//!   见 `docs/git-session-store.md`）；
//! - [`SessionWriter`] / [`JsonlSessionRepo`]：树状 JSONL 文件，append-only
//!   （旧会话的兼容路径，导入后退出）；
//! - [`AgentSessionRepo`]：一个 Agent 的仓库门面 —— 新建走 git，旧会话按 id
//!   分发，列表合并两载体。宿主侧的装配点（`runtime/store.rs`）只认它。

use pipi_protocol::Message;
use pipi_protocol::session::{EntryKind, SessionEntry, kept_start};

/// 会话标题：首个用户消息的首行（截断 48 字符）；没有用户消息时「(空会话)」。
///
/// 两个载体共用这一条规则 —— JSONL 的列表与 git 的列表必须给出同一个标题。
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
    let first_line = text.lines().next().unwrap_or("").trim();
    let mut cut = first_line.chars().take(TITLE_CHARS).collect::<String>();
    if first_line.chars().count() > TITLE_CHARS {
        cut.push('…');
    }
    if cut.is_empty() {
        "(空会话)".to_string()
    } else {
        cut
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
mod history;
mod ledger;
mod repo;
mod store;

pub use git::{GitExec, GitSessionRepo, GitSink, entry_path};
pub use history::load_session;
pub use ledger::SessionWriter;
pub use repo::{JsonlSessionRepo, archive_session_file, fork_session, list_session_summaries};
pub use store::AgentSessionRepo;
