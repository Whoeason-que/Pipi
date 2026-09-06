//! 会话存储：树状 JSONL 条目，append-only，崩溃安全。
//!
//! 移植自 `packages/agent/src/harness/session/types.ts` 的 Entry 模型
//! （EntryBase / MessageEntry）：`id` + `parentId` + `seq` + `timestamp`，
//! 一行一个 JSON 对象。会话是 Agent 的运行日志 —— 只追加、可分叉
//! （parentId 构成树），Pipi 永远不修改已写入的条目。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{now_millis, Message};

/// 条目类型。pi 还有 compaction / branch_summary / custom，先支持两种。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    Message { message: Message },
    Custom { custom_type: String },
}

/// 会话条目。对应 pi 的 `Entry`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub seq: u64,
    pub timestamp: u64,
    #[serde(flatten)]
    pub kind: EntryKind,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 一个会话的追加写入器。`sessions/<millis>-<id>.jsonl`。
pub struct SessionWriter {
    path: PathBuf,
    file: std::fs::File,
    tip_id: Option<String>,
    seq: u64,
}

impl SessionWriter {
    pub fn create(sessions_dir: &Path) -> std::io::Result<SessionWriter> {
        std::fs::create_dir_all(sessions_dir)?;
        let id = new_id();
        let path = sessions_dir.join(format!("{}-{id}.jsonl", now_millis()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(SessionWriter {
            path,
            file,
            tip_id: None,
            seq: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn tip_id(&self) -> Option<&str> {
        self.tip_id.as_deref()
    }

    /// 追加一条消息（挂在当前 tip 上），返回新条目 id。
    pub fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp: message.timestamp(),
            kind: EntryKind::Message {
                message: message.clone(),
            },
        };
        let id = entry.id.clone();
        self.write_entry(&entry)?;
        Ok(id)
    }

    pub fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String> {
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp: now_millis(),
            kind: EntryKind::Custom {
                custom_type: custom_type.to_string(),
            },
        };
        let id = entry.id.clone();
        self.write_entry(&entry)?;
        Ok(id)
    }

    fn write_entry(&mut self, entry: &SessionEntry) -> std::io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?; // 每条即刷，崩溃安全
        self.tip_id = Some(entry.id.clone());
        self.seq += 1;
        Ok(())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentBlock;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pipi-test-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn append_and_load_roundtrip() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();

        let m1 = Message::user_text("hello");
        let m2 = Message::Assistant {
            content: vec![ContentBlock::Text { text: "hi".into() }],
            api: "test".into(),
            provider: "test".into(),
            model: "m".into(),
            usage: Default::default(),
            stop_reason: crate::types::StopReason::Stop,
            error_message: None,
            timestamp: now_millis(),
            duration_ms: None,
        };
        writer.append_message(&m1).unwrap();
        writer.append_message(&m2).unwrap();
        writer.append_custom("bookmark").unwrap();

        let entries = load_session(&writer.path()).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].parent_id.is_none());
        assert_eq!(
            entries[1].parent_id.as_deref(),
            Some(entries[0].id.as_str())
        );
        assert_eq!(
            entries[2].parent_id.as_deref(),
            Some(entries[1].id.as_str())
        );
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[2].seq, 2);
        match &entries[0].kind {
            EntryKind::Message { message } => assert_eq!(message.role(), "user"),
            _ => panic!("expected message entry"),
        }
        match &entries[2].kind {
            EntryKind::Custom { custom_type } => assert_eq!(custom_type, "bookmark"),
            _ => panic!("expected custom entry"),
        }
    }

    #[test]
    fn load_skips_corrupt_lines() {
        let dir = temp_dir();
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
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        writer.append_message(&Message::user_text("b")).unwrap();
        writer.append_message(&Message::user_text("c")).unwrap();

        let entries = load_session(&writer.path()).unwrap();
        let path = active_path(&entries);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].seq, 0);
        assert_eq!(path[2].seq, 2);
    }
}
