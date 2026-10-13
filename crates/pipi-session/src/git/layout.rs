//! tree 路径与 commit 摘要的编码：写侧（sink）与读侧（reader）共用。
//!
//! 一条 entry 一个文件：`e/<3 位分片>/<seq:05>-<kind>-<id8>.json`，内容是该
//! entry 今天在 JSONL 里的那一行原文。分片 = `seq / 1000`（同一分片内 seq 的
//! 位数一致，所以「按文件名排序」与「按 seq 排序」等价，读回不依赖 DAG 拓扑）。

use pipi_protocol::session::{EntryKind, SessionEntry};
use pipi_protocol::{ContentBlock, Message};

/// commit 摘要（`git log --oneline` 的第一行）的字符上限。
const SUMMARY_CHARS: usize = 72;

/// 一个 entry 在 tree 里的路径。
pub fn entry_path(entry: &SessionEntry) -> String {
    let shard = entry.seq / 1000;
    let kind = kind_token(&entry.kind);
    let id8 = id_prefix(&entry.id);
    format!(
        "e/{shard:03}/{:05}-{kind}-{id8}.json",
        entry.seq.min(99_999)
    )
}

/// 解析路径回 `(seq, kind)`；不是本设计写出的路径返回 `None`。
pub fn parse_entry_path(path: &str) -> Option<(u64, String)> {
    let rest = path.strip_prefix("e/")?;
    let (_, file) = rest.split_once('/')?;
    let stem = file.strip_suffix(".json")?;
    let mut parts = stem.splitn(3, '-');
    let seq = parts.next()?.parse::<u64>().ok()?;
    let kind = parts.next()?.to_string();
    Some((seq, kind))
}

/// 该 entry 在 tree 里的 kind 记号（与 serde 的 tag 同名）。
pub fn kind_token(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::Message { .. } => "message",
        EntryKind::Custom { .. } => "custom",
        EntryKind::Compaction { .. } => "compaction",
        EntryKind::ModelChange { .. } => "model_change",
        EntryKind::Env { .. } => "env",
    }
}

/// 该条目是不是用户输入（轮首判据：一轮一个 commit，轮号按提交点递增）。
pub fn is_user_entry(entry: &SessionEntry) -> bool {
    matches!(
        entry.kind,
        EntryKind::Message {
            message: Message::User { .. }
        }
    )
}

/// commit 的 author 名（`<role> <role@pipi.local>` 的 role 部分）。
pub fn entry_role(entry: &SessionEntry) -> &'static str {
    match &entry.kind {
        EntryKind::Message { message } => message.role(),
        other => kind_token(other),
    }
}

/// 单条 entry 的一行摘要（截断到 [`SUMMARY_CHARS`]）。
pub fn summary_line(entry: &SessionEntry) -> String {
    let line = match &entry.kind {
        EntryKind::Message { message } => match message {
            Message::User { content, .. } => format!("user: {}", first_line(content)),
            Message::Assistant { content, .. } => {
                let text = content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                });
                match text {
                    Some(text) if !text.trim().is_empty() => {
                        format!("assistant: {}", first_line(&text))
                    }
                    _ => "assistant: (工具调用)".to_string(),
                }
            }
            Message::ToolResult {
                tool_name, content, ..
            } => {
                let text = content
                    .iter()
                    .map(|part| match part {
                        pipi_protocol::ToolResultContent::Text { text } => text.clone(),
                        pipi_protocol::ToolResultContent::Image { .. } => "[图片]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("toolResult/{tool_name}: {}", first_line(&text))
            }
        },
        EntryKind::Custom { custom_type } => format!("custom: {custom_type}"),
        EntryKind::Compaction { strategy, .. } => format!("compaction: {strategy}"),
        EntryKind::ModelChange { provider } => {
            format!("model_change: {}", provider.display_name())
        }
        EntryKind::Env { declared } => format!("env: {} 项声明", declared.len()),
    };
    truncate_chars(&line, SUMMARY_CHARS)
}

/// 一次提交（一轮）的 commit message：该轮首条 user 消息的一行摘要 +
/// 可选的 `Turn: <n>` trailer。
///
/// 轮号由调用方给（只在该轮含 user 条目时才有）：轮号必须按提交点（= 一次
/// `harvest`）递增，不能按 user 条目数 —— 一轮可以有多条 steer 插话。
pub fn commit_message(batch: &[SessionEntry], turn: Option<u64>) -> String {
    let head = batch
        .iter()
        .find(|entry| {
            matches!(
                entry.kind,
                EntryKind::Message {
                    message: Message::User { .. }
                }
            )
        })
        .or_else(|| batch.first())
        .map(summary_line)
        .unwrap_or_else(|| "(空提交)".to_string());
    let head = truncate_chars(&head, SUMMARY_CHARS);
    match turn {
        Some(turn) => format!("{head}\n\nTurn: {turn}\n"),
        None => format!("{head}\n"),
    }
}

/// 首个非空行（trim 后截断到 [`SUMMARY_CHARS`]）。
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    truncate_chars(line, SUMMARY_CHARS)
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut cut = text.chars().take(max).collect::<String>();
    if text.chars().count() > max {
        cut.push('…');
    }
    cut
}

fn id_prefix(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::session::EnvDeclared;
    use pipi_protocol::{Model, StopReason};

    fn entry(seq: u64, kind: EntryKind) -> SessionEntry {
        SessionEntry {
            id: format!("{:08}abcdef0123456789abcdef0123456", seq),
            parent_id: None,
            seq,
            timestamp: 1_700_000_000_000 + seq,
            kind,
        }
    }

    #[test]
    fn entry_path_roundtrips_and_sorts_by_seq() {
        let a = entry_path(&entry(
            0,
            EntryKind::Custom {
                custom_type: "x".into(),
            },
        ));
        let b = entry_path(&entry(
            1_000,
            EntryKind::Custom {
                custom_type: "x".into(),
            },
        ));
        let c = entry_path(&entry(
            999,
            EntryKind::Custom {
                custom_type: "x".into(),
            },
        ));
        assert_eq!(a, "e/000/00000-custom-00000000.json");
        assert_eq!(c, "e/000/00999-custom-00000999.json");
        assert_eq!(b, "e/001/01000-custom-00001000.json");
        // 文件名排序 == seq 排序（分片按 3 位补零、seq 按 5 位补零）
        assert!(a < c && c < b);
        assert_eq!(parse_entry_path(&a), Some((0, "custom".into())));
        assert_eq!(parse_entry_path(&b), Some((1000, "custom".into())));
        assert_eq!(parse_entry_path("e/000/00000.json"), None);
        assert_eq!(parse_entry_path("other"), None);
    }

    #[test]
    fn summary_lines_cover_every_kind() {
        let user = entry(
            0,
            EntryKind::Message {
                message: Message::user_text("修一下这个 bug\n第二行不该出现"),
            },
        );
        assert_eq!(summary_line(&user), "user: 修一下这个 bug");

        let tool_only = entry(
            1,
            EntryKind::Message {
                message: Message::Assistant {
                    content: vec![ContentBlock::ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: "{}".into(),
                    }],
                    api: "openai-completions".into(),
                    provider: "p".into(),
                    model: "m".into(),
                    usage: Default::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 0,
                    duration_ms: None,
                },
            },
        );
        assert_eq!(summary_line(&tool_only), "assistant: (工具调用)");

        let model = entry(
            2,
            EntryKind::ModelChange {
                provider: Model {
                    id: "claude-sonnet-4-5".into(),
                    name: "Claude Sonnet".into(),
                    api: pipi_protocol::Api::AnthropicMessages,
                    base_url: "https://api.anthropic.com".into(),
                    max_tokens: 8192,
                    context_window: 200_000,
                },
            },
        );
        assert_eq!(summary_line(&model), "model_change: Claude Sonnet");

        let env = entry(
            3,
            EntryKind::Env {
                declared: vec![EnvDeclared {
                    key: "AV_AGENT".into(),
                    source: "runtime".into(),
                }],
            },
        );
        assert_eq!(summary_line(&env), "env: 1 项声明");
    }

    #[test]
    fn commit_message_prefers_the_user_entry_and_carries_the_turn() {
        let model = entry(
            0,
            EntryKind::ModelChange {
                provider: Model {
                    id: "m".into(),
                    name: "Mock".into(),
                    api: pipi_protocol::Api::OpenAICompletions,
                    base_url: "https://example.test".into(),
                    max_tokens: 1,
                    context_window: 1,
                },
            },
        );
        let user = entry(
            1,
            EntryKind::Message {
                message: Message::user_text("开始吧"),
            },
        );
        assert_eq!(
            commit_message(std::slice::from_ref(&model), None),
            "model_change: Mock\n"
        );
        assert_eq!(
            commit_message(&[model, user], Some(3)),
            "user: 开始吧\n\nTurn: 3\n"
        );
    }
}
