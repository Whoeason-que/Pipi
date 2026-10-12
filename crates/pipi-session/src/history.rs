//! JSONL 载体的读侧：整文件读取（坏行跳过）。
//!
//! 条目格式与回放规则（活跃路径、摘要正文、保留区间换算）在
//! `pipi_protocol::session`；这里只负责「文件 → 条目」这一段。

use std::path::Path;

use pipi_protocol::session::SessionEntry;

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

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::Message;
    use pipi_protocol::session::{CompactionRecord, active_path, rebuild_messages, summary_text};

    use crate::SessionWriter;

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

    /// 落盘条目、回放结果与 writer 的 id 映射三者必须一致 ——
    /// 下一次压缩的 `keep_from_entry` 就靠这份一致性换算。
    #[test]
    fn replay_and_writer_mapping_stay_in_sync() {
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

        // 重开后 writer 的映射与回放结果一致（下一次压缩能继续换算 id）
        let reopened = SessionWriter::open(writer.path()).unwrap();
        assert_eq!(reopened.message_ids().len(), messages.len());
        assert_eq!(reopened.message_ids()[1], kept);
    }
}
