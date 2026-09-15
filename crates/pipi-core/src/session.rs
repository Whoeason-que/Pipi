//! 会话存储：树状 JSONL 条目，append-only，崩溃安全。
//!
//! 移植自 `packages/agent/src/harness/session/types.ts` 的 Entry 模型
//! （EntryBase / MessageEntry）：`id` + `parentId` + `seq` + `timestamp`，
//! 一行一个 JSON 对象。会话是 Agent 的运行日志 —— 只追加、可分叉
//! （parentId 构成树），Pipi 永远不修改已写入的条目。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{now_millis, Message, Model};

/// 条目类型。pi 还有 branch_summary / custom；Pipi 增量新增 compaction
/// （摘要式上下文压缩的落点：其之前的消息被摘要替换，回放时丢弃）
/// 与 env（环境契约记账）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    Message { message: Message },
    Custom { custom_type: String },
    #[serde(rename = "model_change")]
    ModelChange {
        #[serde(alias = "model")]
        provider: Model,
    },
    /// 摘要式压缩标记。`summary` 是摘要正文；`source_tip` 是被压缩前的
    /// 活跃条目 id（审计用）。回放时：丢弃此条之前的所有 Message，
    /// 注入一条包裹摘要的 User 消息。
    Compaction {
        summary: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        source_tip: String,
    },
    /// 环境契约记账（Pipi 新增）。记录会话启动时生效的声明式 env 变更：
    /// 键 + 来源层。只记 `declared`（非 process 来源的键）——继承基线不记，
    /// 秘密值永不落盘（契约文件本身在磁盘上，键+来源足以还原语义）。
    /// 回放时只作审计信息，不影响消息流。
    Env { declared: Vec<EnvDeclared> },
}

/// 环境记账单条：变量键 + 来源层标签（"runtime" 或契约层 label）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvDeclared {
    pub key: String,
    pub source: String,
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
    /// 环境记账每会话至多一条；打开旧会话时从已有条目恢复。
    env_recorded: bool,
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
            env_recorded: false,
        })
    }

    /// 打开已有会话文件继续追加（seq/tip 从已有条目恢复）。
    pub fn open(path: &Path) -> std::io::Result<SessionWriter> {
        let entries = load_session(path).map_err(std::io::Error::other)?;
        let (tip_id, seq) = match entries.last() {
            Some(last) => (Some(last.id.clone()), last.seq + 1),
            None => (None, 0),
        };
        let env_recorded = entries
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::Env { .. }));
        let file = std::fs::OpenOptions::new().append(true).open(path)?;
        Ok(SessionWriter {
            path: path.to_path_buf(),
            file,
            tip_id,
            seq,
            env_recorded,
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

    /// 追加一条摘要压缩标记条目，返回新条目 id。
    pub fn append_compaction(&mut self, summary: &str, source_tip: &str) -> std::io::Result<String> {
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp: now_millis(),
            kind: EntryKind::Compaction {
                summary: summary.to_string(),
                source_tip: source_tip.to_string(),
            },
        };
        let id = entry.id.clone();
        self.write_entry(&entry)?;
        Ok(id)
    }

    /// 追加模型变更条目，切换当前会话所用的模型配置。
    pub fn append_model_change(&mut self, model: &Model) -> std::io::Result<String> {
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp: now_millis(),
            kind: EntryKind::ModelChange {
                provider: model.clone(),
            },
        };
        let id = entry.id.clone();
        self.write_entry(&entry)?;
        Ok(id)
    }

    /// 追加环境契约记账条目（每会话至多一条，重复调用幂等跳过），返回新条目 id。
    pub fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>> {
        if self.env_recorded {
            return Ok(None);
        }
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp: now_millis(),
            kind: EntryKind::Env { declared },
        };
        let id = entry.id.clone();
        self.write_entry(&entry)?;
        self.env_recorded = true;
        Ok(Some(id))
    }

    pub fn write_entry(&mut self, entry: &SessionEntry) -> std::io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?; // 每条即刷，崩溃安全
        self.tip_id = Some(entry.id.clone());
        self.seq += 1;
        Ok(())
    }
}

/// 从源会话中分叉出一个新会话文件：
/// 拷贝从根到目标 entry（若为 None 则到当前 tip）的活跃路径条目到新会话文件中。
/// 返回新建且 tip 指向分支末尾的 SessionWriter。
pub fn fork_session(
    source_session_path: &Path,
    sessions_dir: &Path,
    up_to_entry_id: Option<&str>,
) -> Result<SessionWriter, String> {
    let entries = load_session(source_session_path)?;
    if entries.is_empty() {
        return SessionWriter::create(sessions_dir).map_err(|e| e.to_string());
    }
    let active = if let Some(target_id) = up_to_entry_id {
        let by_id: std::collections::HashMap<&str, &SessionEntry> =
            entries.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut path = Vec::new();
        let mut cursor = Some(target_id);
        while let Some(id) = cursor {
            if let Some(entry) = by_id.get(id) {
                path.push((*entry).clone());
                cursor = entry.parent_id.as_deref();
            } else {
                break;
            }
        }
        path.reverse();
        path
    } else {
        active_path(&entries).into_iter().cloned().collect()
    };

    let mut writer = SessionWriter::create(sessions_dir).map_err(|e| e.to_string())?;
    for entry in &active {
        writer.write_entry(entry).map_err(|e| e.to_string())?;
    }
    Ok(writer)
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

/// 会话列表条目（侧栏展示用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// 文件名 stem：`{millis}-{uuid}`，open_session 用它定位文件
    pub id: String,
    /// 标题：首条用户消息截断；空会话给占位文案
    pub title: String,
    pub message_count: usize,
    /// 会话开始时间（文件名里的 millis）
    pub started_at: u64,
    /// 最后一条消息的时间戳
    pub last_active: u64,
    /// 会话使用的模型（从最新 model_change 或最新 assistant 消息提取）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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

/// 扫描会话目录，按最后活跃时间倒序返回摘要。
/// 坏行在 load_session 里跳过，不拖垮列表。
pub fn list_session_summaries(sessions_dir: &Path) -> Vec<SessionSummary> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let id = stem.to_string();
        let started_at = stem
            .split('-')
            .next()
            .and_then(|m| m.parse::<u64>().ok())
            .unwrap_or(0);
        let Ok(file_entries) = load_session(&path) else {
            continue;
        };
        let title = file_entries
            .iter()
            .find_map(|e| match &e.kind {
                EntryKind::Message {
                    message: Message::User { content, .. },
                } => Some(content.clone()),
                _ => None,
            })
            .map(|text| {
                let first_line = text.lines().next().unwrap_or("").trim().to_string();
                let mut cut = first_line.chars().take(48).collect::<String>();
                if first_line.chars().count() > 48 {
                    cut.push('…');
                }
                if cut.is_empty() {
                    "(空会话)".to_string()
                } else {
                    cut
                }
            })
            .unwrap_or_else(|| "(空会话)".to_string());
        let message_count = file_entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Message { .. }))
            .count();
        let last_active = file_entries.last().map(|e| e.timestamp).unwrap_or(0);
        let model = active_model_from_entries(&file_entries)
            .map(|m| m.display_name().to_string())
            .or_else(|| {
                file_entries.iter().rev().find_map(|e| match &e.kind {
                    EntryKind::Message {
                        message: Message::Assistant { model, .. },
                    } if !model.is_empty() => Some(model.clone()),
                    _ => None,
                })
            });
        out.push(SessionSummary {
            id,
            title,
            message_count,
            started_at,
            last_active,
            model,
        });
    }
    out.sort_by_key(|summary| std::cmp::Reverse(summary.last_active));
    out
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

/// 从活跃路径条目重建可执行的消息历史。
///
/// Message 条目顺序累积；遇到 Compaction 条目时丢弃其之前的全部消息，
/// 注入一条包裹摘要的 User 消息（append-only 文件上仍然保留全部原始
/// 条目，分叉/回看不受影响）。open 与 fork 两条重建路径共用此函数。
pub fn rebuild_messages(entries: &[&SessionEntry]) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    for entry in entries {
        match &entry.kind {
            EntryKind::Message { message } => messages.push(message.clone()),
            EntryKind::Compaction { summary, .. } => {
                messages.clear();
                messages.push(Message::user_text(format!(
                    "<compaction summary>\n{summary}\n</compaction summary>"
                )));
            }
            _ => {}
        }
    }
    messages
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
    fn open_resumes_append_position() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("first")).unwrap();
        let path = writer.path().to_path_buf();
        drop(writer);

        // 重新打开：tip/seq 恢复，新条目接在后面
        let mut resumed = SessionWriter::open(&path).unwrap();
        assert_eq!(resumed.tip_id().is_some(), true);
        resumed.append_message(&Message::user_text("second")).unwrap();

        let entries = load_session(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].seq, 1);
        assert_eq!(entries[1].parent_id.as_deref(), Some(entries[0].id.as_str()));
    }

    #[test]
    fn summaries_list_titles_and_order() {
        let dir = temp_dir();
        let mut w1 = SessionWriter::create(&dir).unwrap();
        w1.append_message(&Message::user_text("第一条会话的标题"))
            .unwrap();
        w1.append_message(&Message::user_text("second")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut w2 = SessionWriter::create(&dir).unwrap();
        w2.append_message(&Message::user_text("更新会话")).unwrap();

        let summaries = list_session_summaries(&dir);
        assert_eq!(summaries.len(), 2);
        // 按最后活跃倒序：w2 在前
        assert_eq!(summaries[0].title, "更新会话");
        assert_eq!(summaries[1].title, "第一条会话的标题");
        assert_eq!(summaries[1].message_count, 2);
        assert!(summaries[1].started_at > 0);
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

    #[test]
    fn compaction_entry_roundtrip_and_replay() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("old 1")).unwrap();
        writer.append_message(&Message::user_text("old 2")).unwrap();
        let tip = writer.tip_id().unwrap().to_string();
        writer.append_compaction("此前对话的摘要", &tip).unwrap();
        writer.append_message(&Message::user_text("new turn")).unwrap();

        let entries = load_session(&writer.path()).unwrap();
        // Compaction 条目随 load 原样往返（serde snake_case 兼容）
        assert!(matches!(
            &entries[2].kind,
            EntryKind::Compaction { summary, source_tip }
                if summary == "此前对话的摘要" && source_tip == &tip
        ));

        let active = active_path(&entries);
        let messages = rebuild_messages(&active);
        // 旧消息被摘要替换，摘要之后的新消息保留
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            &messages[0],
            Message::User { content, .. } if content.contains("compaction summary") && content.contains("此前对话的摘要")
        ));
        assert!(matches!(
            &messages[1],
            Message::User { content, .. } if content == "new turn"
        ));
    }

    #[test]
    fn env_entry_roundtrip_idempotent_and_replay_neutral() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("first")).unwrap();

        let declared = vec![
            EnvDeclared {
                key: "AV_AGENT".into(),
                source: "runtime".into(),
            },
            EnvDeclared {
                key: "GITHUB_TOKEN".into(),
                source: "agent.local.toml".into(),
            },
        ];
        assert!(writer.append_env(declared.clone()).unwrap().is_some());
        // 幂等：同一会话再次记账不追加
        assert!(writer.append_env(declared.clone()).unwrap().is_none());
        writer.append_message(&Message::user_text("second")).unwrap();

        let entries = load_session(&writer.path()).unwrap();
        assert!(matches!(
            &entries[1].kind,
            EntryKind::Env { declared: recorded } if recorded == &declared
        ));
        // 打开旧会话：记账状态恢复，再调用仍幂等
        let mut reopened = SessionWriter::open(&writer.path()).unwrap();
        assert!(reopened.append_env(declared).unwrap().is_none());

        // 回放中性：env 条目不影响消息流
        let active = active_path(&entries);
        let messages = rebuild_messages(&active);
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            &messages[0],
            Message::User { content, .. } if content == "first"
        ));
        assert!(matches!(
            &messages[1],
            Message::User { content, .. } if content == "second"
        ));
    }

    #[test]
    fn rebuild_without_compaction_keeps_all_messages() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        writer.append_custom("bookmark").unwrap();
        writer.append_model_change(&Model {
            id: "m".into(),
            name: "m".into(),
            api: crate::types::Api::AnthropicMessages,
            base_url: String::new(),
            max_tokens: 4096,
            context_window: 100_000,
        })
        .unwrap();

        let entries = load_session(&writer.path()).unwrap();
        let active = active_path(&entries);
        let messages = rebuild_messages(&active);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn fork_session_copies_active_history() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        let id1 = writer.append_message(&Message::user_text("msg 1")).unwrap();
        let _id2 = writer.append_message(&Message::user_text("msg 2")).unwrap();
        let source_path = writer.path().to_path_buf();
        drop(writer);

        // 分叉到 id1
        let mut forked = fork_session(&source_path, &dir, Some(&id1)).unwrap();
        assert_eq!(forked.tip_id(), Some(id1.as_str()));
        let id3 = forked.append_message(&Message::user_text("msg 3 branch")).unwrap();
        let forked_entries = load_session(forked.path()).unwrap();
        assert_eq!(forked_entries.len(), 2);
        assert_eq!(forked_entries[1].parent_id.as_deref(), Some(id1.as_str()));
        assert_eq!(forked_entries[1].id, id3);
    }

    #[test]
    fn model_change_entry_roundtrip_and_active_resolution() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        let model1 = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: crate::types::Api::OpenAICompletions,
            base_url: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            context_window: 128000,
        };
        let model2 = Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet".into(),
            api: crate::types::Api::AnthropicMessages,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            context_window: 200000,
        };

        writer.append_message(&Message::user_text("hello")).unwrap();
        writer.append_model_change(&model1).unwrap();
        let id_mid = writer.append_message(&Message::user_text("with gpt-4o")).unwrap();
        writer.append_model_change(&model2).unwrap();
        writer.append_message(&Message::user_text("with claude")).unwrap();

        let entries = load_session(writer.path()).unwrap();
        assert_eq!(entries.len(), 5);

        // 最新活跃模型应为 model2
        let active = active_model_from_entries(&entries).unwrap();
        assert_eq!(active.id, "claude-sonnet-4-5");

        // 分叉到 id_mid，活跃模型应为 model1
        let forked = fork_session(writer.path(), &dir, Some(&id_mid)).unwrap();
        let forked_entries = load_session(forked.path()).unwrap();
        let forked_active = active_model_from_entries(&forked_entries).unwrap();
        assert_eq!(forked_active.id, "gpt-4o");

        let summaries = list_session_summaries(&dir);
        let current_summary = summaries.iter().find(|s| s.id == writer.path().file_stem().unwrap().to_str().unwrap()).unwrap();
        assert_eq!(current_summary.model.as_deref(), Some("Claude Sonnet"));
    }
}

