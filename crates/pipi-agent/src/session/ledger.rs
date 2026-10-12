//! JSONL 追加账本：一条 entry 一行，追加即刷。
//!
//! [`SessionWriter`] 是 [`SessionLog`] 的 JSONL 实现 —— 正式会话写
//! `sessions/<millis>-<id>.jsonl`，设置工作台的临时测试只维护内存账本。
use std::io::Write;
use std::path::{Path, PathBuf};

use pipi_protocol::session::{
    CompactionRecord, EntryKind, EnvDeclared, SessionEntry, SessionLog, active_path, replay,
};
use pipi_protocol::{Message, Model, new_id, now_millis};

use super::{load_session, track_message_ids};

/// 一个会话的追加账本。
///
/// 正式会话把每条 entry 追加到 `sessions/<millis>-<id>.jsonl`；设置工作台的
/// 临时测试会话只维护同样的 tip / seq / message id 映射，不持有文件。这样
/// runtime 的事件、统计与压缩不需要两套状态机，同时保证临时测试不会出现在
/// sessions 目录和会话列表里。
pub struct SessionWriter {
    session_id: String,
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
    tip_id: Option<String>,
    seq: u64,
    /// 环境记账每会话至多一条；打开旧会话时从已有条目恢复。
    env_recorded: bool,
    /// 与内存消息序列一一对应的条目 id（规则见 [`kept_start`]）。
    /// runtime 落盘压缩条目时用它把「保留段下标」换算成 `keep_from_entry`。
    message_ids: Vec<String>,
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
        let session_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| std::io::Error::other("无法解析会话 ID"))?
            .to_string();
        Ok(SessionWriter {
            session_id,
            path: Some(path),
            file: Some(file),
            tip_id: None,
            seq: 0,
            env_recorded: false,
            message_ids: Vec::new(),
        })
    }

    /// 创建不落盘的临时测试账本。ID 仍是稳定的会话身份，供事件过滤、停止、
    /// steering 与供应商缓存路由使用；应用进程退出后它随内存一起消失。
    pub fn temporary() -> SessionWriter {
        SessionWriter {
            session_id: format!("test-{}", new_id()),
            path: None,
            file: None,
            tip_id: None,
            seq: 0,
            env_recorded: false,
            message_ids: Vec::new(),
        }
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
        // 消息 ↔ 条目 id 映射：与 rebuild_messages 同一套回放规则，
        // 保证「内存第 i 条消息」的条目 id 可查。
        let message_ids = replay(&active_path(&entries)).1;
        let file = std::fs::OpenOptions::new().append(true).open(path)?;
        let session_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| std::io::Error::other("无法解析会话 ID"))?
            .to_string();
        Ok(SessionWriter {
            session_id,
            path: Some(path.to_path_buf()),
            file: Some(file),
            tip_id,
            seq,
            env_recorded,
            message_ids,
        })
    }

    /// 正式会话的文件路径。临时账本没有路径；旧的持久化调用点继续使用本方法，
    /// runtime 的通用路径应优先用 [`SessionWriter::session_id`] / `is_temporary`。
    pub fn path(&self) -> &Path {
        self.path.as_deref().expect("临时会话没有持久化路径")
    }

    pub fn persistent_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn is_temporary(&self) -> bool {
        self.path.is_none()
    }

    pub fn tip_id(&self) -> Option<&str> {
        self.tip_id.as_deref()
    }

    /// 当前逻辑消息序列的条目 id（与内存 `messages` 下标一一对应）。
    pub fn message_ids(&self) -> &[String] {
        &self.message_ids
    }

    /// 标记环境记账已存在（分叉拷贝含 Env 条目时用，避免重复记账）。
    pub fn mark_env_recorded(&mut self) {
        self.env_recorded = true;
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

    /// 追加一条摘要压缩条目，返回新条目 id。`record.keep_from_entry` 为空
    /// 表示只保留摘要（旧行为）。
    pub fn append_compaction(&mut self, record: &CompactionRecord<'_>) -> std::io::Result<String> {
        let entry = SessionEntry {
            id: new_id(),
            parent_id: self.tip_id.clone(),
            seq: self.seq,
            timestamp: now_millis(),
            kind: EntryKind::Compaction {
                summary: record.summary.to_string(),
                keep_from_entry: record.keep_from_entry.to_string(),
                source_tip: record.source_tip.to_string(),
                strategy: record.strategy.to_string(),
                usage: record.usage,
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
        if let Some(file) = self.file.as_mut() {
            let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
            line.push('\n');
            file.write_all(line.as_bytes())?;
            file.flush()?; // 每条即刷，崩溃安全
        }
        self.tip_id = Some(entry.id.clone());
        self.seq += 1;
        track_message_ids(&mut self.message_ids, entry);
        Ok(())
    }
}

/// [`SessionLog`] 的 JSONL 实现：转发到同名固有方法。
///
/// `commit_point` 是空操作 —— 每条 entry 落盘即 flush，没有需要固化的缓冲
/// （换载体后才有意义，见 `docs/git-session-store.md`）。
impl SessionLog for SessionWriter {
    fn session_id(&self) -> &str {
        self.session_id()
    }

    fn is_temporary(&self) -> bool {
        self.is_temporary()
    }

    fn tip_id(&self) -> Option<&str> {
        self.tip_id()
    }

    fn message_ids(&self) -> &[String] {
        self.message_ids()
    }

    fn mark_env_recorded(&mut self) {
        self.mark_env_recorded()
    }

    fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        self.append_message(message)
    }

    fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String> {
        self.append_custom(custom_type)
    }

    fn append_compaction(&mut self, record: &CompactionRecord<'_>) -> std::io::Result<String> {
        self.append_compaction(record)
    }

    fn append_model_change(&mut self, model: &Model) -> std::io::Result<String> {
        self.append_model_change(model)
    }

    fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>> {
        self.append_env(declared)
    }

    fn commit_point(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::ContentBlock;
    use pipi_protocol::Usage;
    use pipi_protocol::session::{active_model_from_entries, rebuild_messages, summary_text};

    use crate::session::{fork_session, list_session_summaries};

    #[test]
    fn temporary_writer_keeps_ledger_state_without_a_file() {
        let mut writer = SessionWriter::temporary();
        let session_id = writer.session_id().to_string();
        assert!(session_id.starts_with("test-"));
        assert!(writer.is_temporary());
        assert!(writer.persistent_path().is_none());

        let first = writer
            .append_message(&Message::user_text("只在内存里"))
            .unwrap();
        let second = writer
            .append_message(&Message::assistant_text("收到", "mock"))
            .unwrap();

        assert_eq!(writer.tip_id(), Some(second.as_str()));
        assert_eq!(writer.message_ids(), &[first, second]);
        assert!(writer.persistent_path().is_none());
    }

    #[test]
    fn append_and_load_roundtrip() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();

        let m1 = Message::user_text("hello");
        let m2 = Message::Assistant {
            content: vec![ContentBlock::Text { text: "hi".into() }],
            api: "test".into(),
            provider: "test".into(),
            model: "m".into(),
            usage: Default::default(),
            stop_reason: pipi_protocol::StopReason::Stop,
            error_message: None,
            timestamp: now_millis(),
            duration_ms: None,
        };
        writer.append_message(&m1).unwrap();
        writer.append_message(&m2).unwrap();
        writer.append_custom("bookmark").unwrap();

        let entries = load_session(writer.path()).unwrap();
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
    fn open_resumes_append_position() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("first")).unwrap();
        let path = writer.path().to_path_buf();
        drop(writer);

        // 重新打开：tip/seq 恢复，新条目接在后面
        let mut resumed = SessionWriter::open(&path).unwrap();
        assert_eq!(resumed.tip_id().is_some(), true);
        resumed
            .append_message(&Message::user_text("second"))
            .unwrap();

        let entries = load_session(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].seq, 1);
        assert_eq!(
            entries[1].parent_id.as_deref(),
            Some(entries[0].id.as_str())
        );
    }

    #[test]
    fn compaction_entry_roundtrip_and_replay() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("old 1")).unwrap();
        writer.append_message(&Message::user_text("old 2")).unwrap();
        let tip = writer.tip_id().unwrap().to_string();
        // 旧式落盘（无 keep_from_entry）：只保留摘要
        writer
            .append_compaction(&CompactionRecord {
                summary: "此前对话的摘要",
                strategy: "llm-summarize",
                keep_from_entry: "",
                source_tip: &tip,
                usage: None,
            })
            .unwrap();
        writer
            .append_message(&Message::user_text("new turn"))
            .unwrap();

        let entries = load_session(writer.path()).unwrap();
        // Compaction 条目随 load 原样往返（serde snake_case 兼容）
        assert!(matches!(
            &entries[2].kind,
            EntryKind::Compaction { summary, source_tip, strategy, keep_from_entry, .. }
                if summary == "此前对话的摘要"
                    && source_tip == &tip
                    && strategy == "llm-summarize"
                    && keep_from_entry.is_empty()
        ));

        let active = active_path(&entries);
        let messages = rebuild_messages(&active);
        // 旧消息被摘要替换，摘要之后的新消息保留
        assert_eq!(messages.len(), 2);
        assert_eq!(summary_text(&messages[0]), Some("此前对话的摘要"));
        assert!(matches!(
            &messages[1],
            Message::User { content, .. } if content == "new turn"
        ));
    }

    /// 回归：`keep_from_entry` 起的那段原文必须活过重开 —— 内存里保留了什么，
    /// 回放后就该是什么（此前回放只留摘要，重开后那段精确原文就丢了）。

    #[test]
    fn compaction_entry_persists_summary_usage() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        let tip = writer.tip_id().unwrap().to_string();
        let usage = Usage {
            input: 40_000,
            output: 800,
            cache_read: 0,
            cache_write: 0,
            total_tokens: 40_800,
        };
        writer
            .append_compaction(&CompactionRecord {
                summary: "摘要",
                strategy: "llm-summarize",
                keep_from_entry: "",
                source_tip: &tip,
                usage: Some(usage),
            })
            .unwrap();
        let entries = load_session(writer.path()).unwrap();
        assert!(matches!(
            &entries[1].kind,
            EntryKind::Compaction { usage: Some(recorded), .. } if *recorded == usage
        ));
    }

    #[test]
    fn env_entry_roundtrip_idempotent_and_replay_neutral() {
        let dir = pipi_test_support::temp_dir("pipi-test");
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
        writer
            .append_message(&Message::user_text("second"))
            .unwrap();

        let entries = load_session(writer.path()).unwrap();
        assert!(matches!(
            &entries[1].kind,
            EntryKind::Env { declared: recorded } if recorded == &declared
        ));
        // 打开旧会话：记账状态恢复，再调用仍幂等
        let mut reopened = SessionWriter::open(writer.path()).unwrap();
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
    fn model_change_entry_roundtrip_and_active_resolution() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        let model1 = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            context_window: 128000,
        };
        let model2 = Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet".into(),
            api: pipi_protocol::Api::AnthropicMessages,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            context_window: 200000,
        };

        writer.append_message(&Message::user_text("hello")).unwrap();
        writer.append_model_change(&model1).unwrap();
        let id_mid = writer
            .append_message(&Message::user_text("with gpt-4o"))
            .unwrap();
        writer.append_model_change(&model2).unwrap();
        writer
            .append_message(&Message::user_text("with claude"))
            .unwrap();

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
        let current_summary = summaries
            .iter()
            .find(|s| s.id == writer.path().file_stem().unwrap().to_str().unwrap())
            .unwrap();
        assert_eq!(current_summary.model.as_deref(), Some("Claude Sonnet"));
    }
}
