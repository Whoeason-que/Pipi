//! 旧 JSONL 的读侧：整文件逐行读取（坏行跳过）。
//!
//! 条目格式与回放规则（活跃路径、摘要正文、保留区间换算）在
//! `pipi_protocol::session`；这里只负责「文件 → 条目」这一段。单轨化后运行时
//! 不再直接读会话文件 —— 它服务显式导入（[`crate::import_legacy_sessions`]）
//! 与按单条 entry blob 读回的 git 读侧（同一套逐行解析口径）。

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
    use pipi_protocol::session::EntryKind;

    /// 直接写一行 JSON（旧格式就是这一行）；不依赖任何载体实现。
    fn write_lines(path: &Path, lines: &[String]) {
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    fn message_entry(id: &str, parent: Option<&str>, seq: u64, text: &str) -> SessionEntry {
        SessionEntry {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            seq,
            timestamp: seq,
            kind: EntryKind::Message {
                message: Message::user_text(text),
            },
        }
    }

    #[test]
    fn load_skips_corrupt_lines() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let path = dir.join("session.jsonl");
        let good = serde_json::to_string(&message_entry("m1", None, 0, "good")).unwrap();
        // 模拟崩溃留下的半行
        write_lines(&path, &[good, r#"{"id":"broken""#.into()]);

        let entries = load_session(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            &entries[0].kind,
            EntryKind::Message { message } if message.role() == "user"
        ));
    }

    #[test]
    fn load_reads_a_whole_append_only_file_in_order() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let path = dir.join("session.jsonl");
        let first = message_entry("m1", None, 0, "第一句");
        let second = message_entry("m2", Some("m1"), 1, "第二句");
        let lines = vec![
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap(),
        ];
        write_lines(&path, &lines);

        let entries = load_session(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].parent_id.as_deref(), Some("m1"));
        assert_eq!(entries[1].seq, 1);
    }
}
