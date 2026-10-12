//! JSONL 载体的读侧：整文件读取、活跃路径、回放与摘要正文。
//!
//! 这些规则与载体无关（换后端也必须一致），所以保持自由函数：条目进、消息出。
use super::*;

/// 读取整个会话文件。坏行跳过（崩溃安全：绝不让半行 JSON 拖垮整个会话）。
pub fn load_session(path: &Path) -> Result<Vec<SessionEntry>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => eprintln!(
                "pipi: 会话文件 {} 第 {} 行损坏，已跳过: {e}",
                path.display(),
                i + 1
            ),
        }
    }
    Ok(entries)
}

/// 从条目列表中检索活跃路径上最新的模型配置（若无则返回 None，由上层回退到 Agent 默认模型）。
pub fn active_model_from_entries(entries: &[SessionEntry]) -> Option<Model> {
    for entry in active_path(entries).into_iter().rev() {
        if let EntryKind::ModelChange { provider } = &entry.kind {
            return Some(provider.clone());
        }
    }
    None
}

/// 重建「活跃路径」：从 tip 沿 parentId 回溯到根（pi 的树状回放）。
pub fn active_path(entries: &[SessionEntry]) -> Vec<&SessionEntry> {
    use std::collections::HashMap;
    let by_id: HashMap<&str, &SessionEntry> = entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut path = Vec::new();
    let mut cursor = entries.last().map(|e| e.id.as_str());
    while let Some(id) = cursor {
        if let Some(entry) = by_id.get(id) {
            path.push(*entry);
            cursor = entry.parent_id.as_deref();
        } else {
            break;
        }
    }
    path.reverse();
    path
}

/// 摘要消息的包裹标签（会话文件与请求里都是这一种形状）。
pub const SUMMARY_OPEN_TAG: &str = "<compaction summary>";
pub const SUMMARY_CLOSE_TAG: &str = "</compaction summary>";

/// 把摘要正文包装成注入历史的那条 User 消息（pi 同款形状）。
pub fn summary_message(summary: &str) -> Message {
    Message::user_text(format!(
        "{SUMMARY_OPEN_TAG}\n{summary}\n{SUMMARY_CLOSE_TAG}"
    ))
}

/// 从一条消息里取回摘要正文（不是摘要消息则 `None`）。
pub fn summary_text(message: &Message) -> Option<&str> {
    let Message::User { content, .. } = message else {
        return None;
    };
    let rest = content.strip_prefix(SUMMARY_OPEN_TAG)?.strip_prefix('\n')?;
    rest.split(SUMMARY_CLOSE_TAG)
        .next()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
}

/// 从活跃路径条目回放可执行的消息历史，并给出与之一一对应的条目 id。
///
/// - `Message` 条目顺序累积；
/// - `Compaction` 条目**丢弃其之前的消息**，注入一条摘要 User 消息，并保留
///   `keep_from_entry` 起的原文（append-only 文件上仍保留全部原始条目，
///   分叉/回看不受影响）；定位不到起点时只留摘要（旧条目的行为）；
/// - 返回的 id 序列里，摘要消息对应压缩条目自身的 id —— runtime 据此把
///   「保留段下标」换算成下一次压缩的 `keep_from_entry`。
///
/// open / fork / writer 的映射维护共用此函数，规则只有一份。
pub fn replay(entries: &[&SessionEntry]) -> (Vec<Message>, Vec<String>) {
    let mut messages: Vec<Message> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        match &entry.kind {
            EntryKind::Message { message } => {
                messages.push(message.clone());
                ids.push(entry.id.clone());
            }
            EntryKind::Compaction {
                summary,
                keep_from_entry,
                ..
            } => {
                let at = kept_start(&ids, keep_from_entry);
                let mut kept_messages = messages.split_off(at);
                let mut kept_ids = ids.split_off(at);
                messages.clear();
                messages.push(summary_message(summary));
                messages.append(&mut kept_messages);
                ids.clear();
                ids.push(entry.id.clone());
                ids.append(&mut kept_ids);
            }
            _ => {}
        }
    }
    (messages, ids)
}

/// 从活跃路径条目重建可执行的消息历史（见 [`replay`]）。
pub fn rebuild_messages(entries: &[&SessionEntry]) -> Vec<Message> {
    replay(entries).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_skips_corrupt_lines() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("good")).unwrap();
        let path = writer.path().to_path_buf();
        // 模拟崩溃留下的半行
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, r#"{{"id":"broken""#).unwrap();
        drop(f);

        let entries = load_session(&path).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn active_path_walks_parents() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        writer.append_message(&Message::user_text("b")).unwrap();
        writer.append_message(&Message::user_text("c")).unwrap();

        let entries = load_session(writer.path()).unwrap();
        let path = active_path(&entries);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].seq, 0);
        assert_eq!(path[2].seq, 2);
    }

    #[test]
    fn replay_keeps_messages_from_first_kept_entry() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer
            .append_message(&Message::user_text("turn 1"))
            .unwrap();
        writer
            .append_message(&Message::user_text("turn 2"))
            .unwrap();
        let kept = writer
            .append_message(&Message::user_text("turn 3"))
            .unwrap();
        writer
            .append_message(&Message::user_text("turn 4"))
            .unwrap();
        let tip = writer.tip_id().unwrap().to_string();
        // 内存历史：[1,2,3,4]，保留区间从 index 2（turn 3）开始
        assert_eq!(writer.message_ids().len(), 4);
        assert_eq!(writer.message_ids()[2], kept);
        writer
            .append_compaction(&CompactionRecord {
                summary: "前两轮摘要",
                strategy: "llm-summarize",
                keep_from_entry: &kept,
                source_tip: &tip,
                usage: None,
            })
            .unwrap();
        writer
            .append_message(&Message::user_text("turn 5"))
            .unwrap();

        let entries = load_session(writer.path()).unwrap();
        let active = active_path(&entries);
        let messages = rebuild_messages(&active);
        // 摘要 + 保留的 turn 3/4 + 压缩后的 turn 5
        assert_eq!(messages.len(), 4);
        assert_eq!(summary_text(&messages[0]), Some("前两轮摘要"));
        let texts: Vec<&str> = messages
            .iter()
            .skip(1)
            .map(|message| match message {
                Message::User { content, .. } => content.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(texts, vec!["turn 3", "turn 4", "turn 5"]);

        // 重开后 writer 的映射与回放结果一致（下一次压缩能继续换算 id）
        let reopened = SessionWriter::open(writer.path()).unwrap();
        assert_eq!(reopened.message_ids().len(), messages.len());
        assert_eq!(reopened.message_ids()[1], kept);
    }

    /// 定位不到 keep_from_entry（条目被分叉截断等）时退回「只留摘要」，
    /// 不猜位置、不产生无主消息。

    #[test]
    fn replay_falls_back_to_summary_only_when_kept_entry_is_missing() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        let tip = writer.tip_id().unwrap().to_string();
        writer
            .append_compaction(&CompactionRecord {
                summary: "摘要",
                strategy: "llm-summarize",
                keep_from_entry: "不存在的条目 id",
                source_tip: &tip,
                usage: None,
            })
            .unwrap();
        let entries = load_session(writer.path()).unwrap();
        let messages = rebuild_messages(&active_path(&entries));
        assert_eq!(messages.len(), 1);
        assert_eq!(summary_text(&messages[0]), Some("摘要"));
    }

    /// 摘要正文的包裹与取回是同一套规则（落盘存原文，回放时包裹）。

    #[test]
    fn summary_message_roundtrip_is_not_nested() {
        let message = summary_message("## Goal\n做事");
        assert_eq!(summary_text(&message), Some("## Goal\n做事"));
        assert_eq!(summary_text(&Message::user_text("普通消息")), None);
        // 二次包裹仍能取回同一份正文（不会套娃）
        let again = summary_message(summary_text(&message).unwrap());
        assert_eq!(summary_text(&again), Some("## Goal\n做事"));
    }

    #[test]
    fn rebuild_without_compaction_keeps_all_messages() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        writer.append_custom("bookmark").unwrap();
        writer
            .append_model_change(&Model {
                id: "m".into(),
                name: "m".into(),
                api: pipi_protocol::Api::AnthropicMessages,
                base_url: String::new(),
                max_tokens: 4096,
                context_window: 100_000,
            })
            .unwrap();

        let entries = load_session(writer.path()).unwrap();
        let active = active_path(&entries);
        let messages = rebuild_messages(&active);
        assert_eq!(messages.len(), 1);
    }
}
