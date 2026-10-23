//! `session_search`：只读的历史会话检索（同一 Agent 的活跃区与归档区）。
//!
//! 三种形态由参数决定，与 hermes 的 discovery / read / browse 对齐，但没有
//! 滚动窗口与血缘去重（Pipi 的压缩分叉会把原会话归档，检索直接覆盖归档区即可）：
//!
//! - 无参 → **列表**：最近的会话（id、标题、时间、条数、是否归档）。
//! - `query` → **检索**：大小写不敏感的子串匹配，每个会话只返回第一处命中的片段。
//! - `sessionId` → **读取**：该会话的用户/助手文本，按 `offset` 分页。
//!
//! 只读：只调用 [`SessionRepo`] 的读取方法，不写任何载体。防浪费：结果数、单段
//! 字符数与总字符预算都有上限，超出即标注 `truncated` 让模型自行缩小范围。
//! 当前会话默认排除（它已经在上下文里）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use pipi_protocol::session::{EntryKind, SessionEntry, SessionRepo, SessionSummary};
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};
use pipi_protocol::{ContentBlock, Message, ToolResultContent};

/// 列表 / 检索返回的会话数上限。
pub const MAX_SESSIONS: usize = 10;
/// 检索时单个会话命中片段的字符上限。
pub const MAX_SNIPPET_CHARS: usize = 300;
/// 读取时单条消息的字符上限。
pub const MAX_MESSAGE_CHARS: usize = 1_200;
/// 单次工具输出的总字符预算。
pub const MAX_OUTPUT_CHARS: usize = 8_000;

pub struct SessionSearchTool {
    repo: Arc<dyn SessionRepo>,
    /// 当前会话 id：检索与列表默认排除它（已在上下文里）。
    current_session_id: Option<String>,
}

impl SessionSearchTool {
    pub fn new(repo: Arc<dyn SessionRepo>, current_session_id: Option<String>) -> Self {
        Self {
            repo,
            current_session_id,
        }
    }
}

#[async_trait]
impl AgentTool for SessionSearchTool {
    fn name(&self) -> &'static str {
        "session_search"
    }

    fn description(&self) -> String {
        "Search or read this agent's past conversations (including archived ones that were \
         replaced by a compaction). No args: list recent sessions. `query`: find sessions whose \
         messages contain the text and show the first match in each. `sessionId`: read that \
         session's messages (use `offset` to page). Read-only. Use only when the user refers to \
         something from a past conversation, or the current context lacks information you \
         suspect was discussed before; prefer a narrow query over reading whole sessions."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Text to find in past messages (case-insensitive substring)."
                },
                "sessionId": {
                    "type": "string",
                    "description": "Read this session's messages. Ids come from a list or query result."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "With sessionId: index of the first message to return (default 0)."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SESSIONS,
                    "description": "Max sessions for list/query (default 5, max 10)."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let text = if let Some(session_id) = non_empty(args, "sessionId") {
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            read_session(self.repo.as_ref(), session_id, offset)?
        } else {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, MAX_SESSIONS))
                .unwrap_or(5);
            let candidates = self.candidates();
            match non_empty(args, "query") {
                Some(query) => search(self.repo.as_ref(), &candidates, query, limit),
                None => list(&candidates, limit),
            }
        };
        Ok(ToolOutput {
            content: vec![ToolResultContent::Text { text }],
            details: None,
            terminate: false,
        })
    }
}

impl SessionSearchTool {
    /// 候选会话：活跃区 + 归档区，排除当前会话，按最后活跃时间倒序。
    /// 归档区读取失败只降级（不影响活跃区结果）。
    fn candidates(&self) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = self
            .repo
            .list()
            .into_iter()
            .map(|summary| Candidate::new(summary, false))
            .collect();
        if let Ok(archived) = self.repo.list_archived() {
            out.extend(
                archived
                    .into_iter()
                    .map(|summary| Candidate::new(summary, true)),
            );
        }
        out.retain(|c| self.current_session_id.as_deref() != Some(c.summary.id.as_str()));
        out.sort_by(|a, b| b.summary.last_active.cmp(&a.summary.last_active));
        out
    }
}

struct Candidate {
    summary: SessionSummary,
    archived: bool,
}

impl Candidate {
    fn new(summary: SessionSummary, archived: bool) -> Self {
        Self { summary, archived }
    }
}

fn non_empty<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 列表形态：最近的会话元数据，不读消息正文。
fn list(candidates: &[Candidate], limit: usize) -> String {
    if candidates.is_empty() {
        return "No historical sessions available.".into();
    }
    let mut lines = vec![format!("Recent historical sessions (up to {limit}):")];
    for candidate in candidates.iter().take(limit) {
        lines.push(format_meta(candidate));
    }
    if candidates.len() > limit {
        lines.push(format!(
            "({} more not listed, increase limit or use query to narrow)",
            candidates.len() - limit
        ));
    }
    lines.join("\n")
}

fn format_meta(candidate: &Candidate) -> String {
    let s = &candidate.summary;
    format!(
        "- {}{}: {} ({} messages)",
        s.id,
        if candidate.archived {
            " [archived]"
        } else {
            ""
        },
        s.title,
        s.message_count
    )
}

/// 检索形态：逐会话扫描，命中即记录第一处片段并跳到下一个会话。
fn search(repo: &dyn SessionRepo, candidates: &[Candidate], query: &str, limit: usize) -> String {
    let needle = query.to_lowercase();
    let mut hits: Vec<String> = Vec::new();
    let mut total_chars = 0usize;
    let mut truncated = false;
    let mut scanned_failed = 0usize;

    for candidate in candidates {
        if hits.len() >= limit {
            break;
        }
        let entries = match load(repo, candidate) {
            Ok(entries) => entries,
            Err(_) => {
                scanned_failed += 1;
                continue;
            }
        };
        let Some((index, snippet)) = first_match(&entries, &needle) else {
            continue;
        };
        let block = format!(
            "{}\n  Message #{}: {}",
            format_meta(candidate),
            index,
            snippet
        );
        if total_chars + block.chars().count() > MAX_OUTPUT_CHARS {
            truncated = true;
            break;
        }
        total_chars += block.chars().count();
        hits.push(block);
    }

    if hits.is_empty() {
        let mut out = format!("No historical sessions found containing \"{query}\".");
        if scanned_failed > 0 {
            out.push_str(&format!(" ({scanned_failed} sessions unreadable, skipped)"));
        }
        return out;
    }
    let mut out = vec![format!("Historical sessions containing \"{query}\":")];
    out.extend(hits);
    if truncated {
        out.push("(Output reached character budget, results truncated; please use a more specific query)".into());
    }
    out.join("\n")
}

/// 读取形态：按 `offset` 返回一段消息，每条截断，总字符受预算限制。
fn read_session(repo: &dyn SessionRepo, session_id: &str, offset: usize) -> Result<String, String> {
    let entries = load_by_id(repo, session_id)?;
    let messages = active_messages(&entries);
    if messages.is_empty() {
        return Ok(format!("Session {session_id} has no messages."));
    }
    if offset >= messages.len() {
        return Ok(format!(
            "Session {session_id} has {} messages in total; offset {offset} is out of range.",
            messages.len()
        ));
    }
    let mut out = vec![format!(
        "Session {session_id} (from #{offset}, {} total):",
        messages.len()
    )];
    let mut total = out[0].chars().count();
    let mut next = offset;
    for (index, message) in messages.iter().enumerate().skip(offset) {
        let line = format!("[{index}] {}", render(message, MAX_MESSAGE_CHARS));
        if total + line.chars().count() > MAX_OUTPUT_CHARS {
            break;
        }
        total += line.chars().count();
        out.push(line);
        next = index + 1;
    }
    if next < messages.len() {
        out.push(format!("(Truncated; use offset={next} to continue reading)"));
    }
    Ok(out.join("\n"))
}

fn load(repo: &dyn SessionRepo, candidate: &Candidate) -> Result<Vec<SessionEntry>, String> {
    if candidate.archived {
        repo.entries_archived(&candidate.summary.id)
    } else {
        repo.entries(&candidate.summary.id)
    }
}

fn load_by_id(repo: &dyn SessionRepo, session_id: &str) -> Result<Vec<SessionEntry>, String> {
    // 先活跃区，再归档区：同一 id 只会在其中一处
    match repo.entries(session_id) {
        Ok(entries) => Ok(entries),
        Err(_) => repo.entries_archived(session_id),
    }
}

/// 活跃路径上的消息（压缩之后的上下文视图之前的原始消息，过滤掉非消息条目）。
fn active_messages(entries: &[SessionEntry]) -> Vec<Message> {
    let path = pipi_protocol::session::active_path(entries);
    path.into_iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// 第一处命中：返回（消息序号，截断后的片段）。只看用户与助手的文本。
fn first_match(entries: &[SessionEntry], needle: &str) -> Option<(usize, String)> {
    let messages = active_messages(entries);
    for (index, message) in messages.iter().enumerate() {
        let text = plain_text(message);
        let lower = text.to_lowercase();
        if let Some(byte_at) = lower.find(needle) {
            return Some((index, snippet_around(&text, byte_at, needle.len())));
        }
    }
    None
}

fn plain_text(message: &Message) -> String {
    match message {
        Message::User { content, .. } => content.clone(),
        Message::Assistant { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        Message::ToolResult { .. } => String::new(),
    }
}

/// 以命中位置为中心截取片段（按字符边界，避免切断 UTF-8）。
fn snippet_around(text: &str, byte_at: usize, needle_len: usize) -> String {
    let start_chars = text[..byte_at].chars().count();
    let total_chars = text.chars().count();
    let half = MAX_SNIPPET_CHARS.saturating_sub(needle_len.min(MAX_SNIPPET_CHARS)) / 2;
    let start = start_chars.saturating_sub(half);
    let end = (start + MAX_SNIPPET_CHARS).min(total_chars);
    let body: String = text.chars().skip(start).take(end - start).collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < total_chars { "…" } else { "" };
    format!("{prefix}{}{suffix}", body.replace('\n', " "))
}

/// 读取形态的单条渲染：工具结果只给占位，文本按字符截断。
fn render(message: &Message, max_chars: usize) -> String {
    let (role, text) = match message {
        Message::User { content, .. } => ("User", content.clone()),
        Message::Assistant { .. } => ("Assistant", plain_text(message)),
        Message::ToolResult { tool_name, .. } => {
            return format!("(Output of tool {tool_name}, omitted)");
        }
    };
    let mut body: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        body.push('…');
    }
    format!("{role}: {body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_is_char_safe_and_centered() {
        let text = "前缀".repeat(200) + "目标词" + &"后缀".repeat(200);
        let at = text.find("目标词").unwrap();
        let snippet = snippet_around(&text, at, "目标词".len());
        assert!(snippet.contains("目标词"));
        assert!(snippet.chars().count() <= MAX_SNIPPET_CHARS + 2);
    }

    #[test]
    fn render_truncates_long_messages_and_hides_tool_output() {
        let long = Message::user_text("字".repeat(5_000));
        let rendered = render(&long, 100);
        assert!(rendered.ends_with('…'));
        assert!(rendered.chars().count() <= 100 + "User: …".chars().count());

        let tool = Message::ToolResult {
            tool_call_id: "t".into(),
            tool_name: "bash".into(),
            content: vec![ToolResultContent::Text {
                text: "x".repeat(9_000),
            }],
            is_error: false,
            details: None,
            timestamp: 0,
        };
        assert!(render(&tool, 100).contains("omitted"));
    }
}
