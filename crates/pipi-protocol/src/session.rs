//! 会话的格式与语义：树状条目、回放规则，以及存储端口。
//!
//! 移植自 `packages/agent/src/harness/session/types.ts` 的 Entry 模型
//! （EntryBase / MessageEntry）：`id` + `parentId` + `seq` + `timestamp`，
//! 一行一个 JSON 对象。会话是 Agent 的运行日志 —— 只追加、可分叉
//! （parentId 构成树），Pipi 永远不修改已写入的条目。
//!
//! **格式、语义与载体分离**：本模块的 [`SessionEntry`] / [`EntryKind`] /
//! [`replay`] 等对所有载体一致；落盘动作走两个端口 —— [`SessionLog`]
//! （追加账本，运行时持有）与 [`SessionRepo`]（目录级操作：建/开/列/分叉/
//! 归档/删）。端口属主是协议层，实现（JSONL 文件、git 分支）在宿主侧的载体
//! crate；换载体只换实现，条目格式与 IPC 契约不变
//! （设计见 `docs/git-session-store.md`）。

use serde::{Deserialize, Serialize};

use crate::{Message, Model, Usage};

/// 条目类型。pi 还有 branch_summary / custom；Pipi 增量新增 compaction
/// （摘要式上下文压缩的落点：其之前的消息被摘要替换，回放时丢弃）
/// 与 env（环境契约记账）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    Message {
        message: Message,
    },
    Custom {
        custom_type: String,
    },
    #[serde(rename = "model_change")]
    ModelChange {
        #[serde(alias = "model")]
        provider: Model,
    },
    /// 摘要式压缩标记。`summary` 是摘要正文；`keep_from_entry` 是**保留区间
    /// 的起点条目 id**（对齐 pi 的 `firstKeptEntryId`）—— 回放时该条目及其之后
    /// 的消息按原文保留，之前的丢弃；`source_tip` 是压缩前的活跃条目 id
    /// （审计用）；`usage` 是生成摘要那次调用的用量（会话账本要计入）；
    /// `strategy` 是产出它的策略名（诊断用）。
    ///
    /// `keep_from_entry` 为空（旧条目、或落盘时无法定位）时退回旧行为：
    /// 只保留摘要。
    Compaction {
        summary: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        keep_from_entry: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        source_tip: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        strategy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// 环境契约记账（Pipi 新增）。记录会话启动时生效的声明式 env 变更：
    /// 键 + 来源层。只记 `declared`（非 process 来源的键）——继承基线不记，
    /// 秘密值永不落盘（契约文件本身在磁盘上，键+来源足以还原语义）。
    /// 回放时只作审计信息，不影响消息流。
    Env {
        declared: Vec<EnvDeclared>,
    },
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

/// 归档子目录名。Agent 数据树里两处归档共用这一个名字：整个 Agent 的归档
/// （`agents/.archive/<name>/`）与会话文件的归档（`sessions/.archive/<id>.jsonl`）。
pub const ARCHIVE_DIR: &str = ".archive";

/// 一次摘要压缩要落盘的内容。策略产出，runtime 交给 writer。
pub struct CompactionRecord<'a> {
    pub summary: &'a str,
    /// 策略名（诊断用，例如 "llm-summarize"）。
    pub strategy: &'a str,
    /// 保留区间起点的条目 id；空串 = 只保留摘要。
    pub keep_from_entry: &'a str,
    /// 压缩前的活跃条目 id（审计用）。
    pub source_tip: &'a str,
    /// 生成摘要那次调用的用量（计入会话账本）。
    pub usage: Option<Usage>,
}

impl CompactionRecord<'_> {
    /// 落成条目 payload（各载体的 `append_compaction` 共用，字段搬运只有这一份）。
    pub fn to_entry_kind(&self) -> EntryKind {
        EntryKind::Compaction {
            summary: self.summary.to_string(),
            keep_from_entry: self.keep_from_entry.to_string(),
            source_tip: self.source_tip.to_string(),
            strategy: self.strategy.to_string(),
            usage: self.usage,
        }
    }
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

/// 显式导入（旧 JSONL 会话 → git 分支）的结果：宿主命令回报给用户。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionImportReport {
    /// 新导入的会话 id。
    pub imported: Vec<String>,
    /// 已在目标载体里且内容一致的会话 id（幂等跳过）。
    pub skipped: Vec<String>,
    /// 失败的会话与原因。
    pub failed: Vec<SessionImportFailure>,
}

/// 一条导入失败记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionImportFailure {
    pub session_id: String,
    pub reason: String,
}

/// 会话 ID 校验：只允许 ASCII 字母数字与 `-`。
///
/// 会话 id 同时是文件名（JSONL 载体）与分支名（git 载体），必须拒绝
/// 路径分隔符、`.` 与空串；所有载体共用这一条规则，别在调用点各写一份。
pub fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("非法会话 ID".into());
    }
    Ok(())
}

/// 会话追加账本：运行时只依赖这个接口，载体由宿主注入。
///
/// 身份与记账（`tip_id` / `message_ids`）留在接口上：事件过滤、停止、steering、
/// 供应商缓存路由与压缩的 `keep_from_entry` 换算都要用，且与载体无关。
pub trait SessionLog: Send {
    /// 会话身份。稳定不变，直到压缩分叉换出新会话。
    fn session_id(&self) -> &str;
    /// 临时测试会话（设置工作台）：不落盘，也不能分叉/归档。
    fn is_temporary(&self) -> bool;
    /// 当前活跃条目 id（下一条追加的挂载点）。
    fn tip_id(&self) -> Option<&str>;
    /// 与内存消息序列一一对应的条目 id（规则见 [`replay`]）。
    fn message_ids(&self) -> &[String];
    /// 标记环境记账已存在（分叉拷贝含 Env 条目时用，避免重复记账）。
    fn mark_env_recorded(&mut self);
    /// 追加一条消息（挂在当前 tip 上），返回新条目 id。
    fn append_message(&mut self, message: &Message) -> std::io::Result<String>;
    fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String>;
    fn append_compaction(&mut self, record: &CompactionRecord<'_>) -> std::io::Result<String>;
    fn append_model_change(&mut self, model: &Model) -> std::io::Result<String>;
    /// 追加环境契约记账条目（每会话至多一条，重复调用幂等跳过）。
    fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>>;
    /// 提交点：把尚未固化的条目落定。
    ///
    /// 时机由运行时决定（今天的口径是每个 turn 结束，见
    /// `docs/git-session-store.md`）。JSONL 实现逐条即刷，所以是空操作；
    /// 缓冲式载体（如 git 分支）在这里写一次提交。
    fn commit_point(&mut self) -> std::io::Result<()>;
}

/// 会话仓库：目录级操作（建 / 开 / 列 / 分叉 / 归档 / 删）。按 Agent 构造。
///
/// 运行时的会话生命周期操作全部走这里 —— 换载体只换实现，调用点不动。
pub trait SessionRepo: Send + Sync {
    /// 新建一条会话，返回它的账本。
    fn create(&self) -> Result<Box<dyn SessionLog>, String>;
    /// 打开已有会话继续追加。
    fn open(&self, session_id: &str) -> Result<Box<dyn SessionLog>, String>;
    /// 会话是否存在于活跃区（供「先检查、再进临界区」的调用点）。
    ///
    /// 实现必须对符号链接 fail-closed（返回 `Err`）—— 读、写、分叉都不得
    /// 跟随链接逃出 Agent 数据树。
    fn exists(&self, session_id: &str) -> Result<bool, String>;
    /// 读回一个会话的全部条目（含非活跃路径）。坏行跳过。
    fn entries(&self, session_id: &str) -> Result<Vec<SessionEntry>, String>;
    /// 活跃区会话摘要，按最后活跃时间倒序。
    fn list(&self) -> Vec<SessionSummary>;
    /// 归档区会话摘要。
    fn list_archived(&self) -> Result<Vec<SessionSummary>, String>;
    /// 从源会话分叉出新会话（`up_to_entry_id` 为空则到当前 tip）。
    fn fork(
        &self,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<Box<dyn SessionLog>, String>;
    /// 移入归档区。
    fn archive(&self, session_id: &str) -> Result<(), String>;
    /// 从归档区移回活跃区。
    fn restore(&self, session_id: &str) -> Result<(), String>;
    /// 彻底删除活跃区会话。不可恢复。
    fn delete(&self, session_id: &str) -> Result<(), String>;
    /// 彻底删除归档区会话。不可恢复。
    fn delete_archived(&self, session_id: &str) -> Result<(), String>;
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
const SUMMARY_OPEN_TAG: &str = "<compaction summary>";
const SUMMARY_CLOSE_TAG: &str = "</compaction summary>";

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

/// 保留区间在 id 序列里的起点下标。空 id 或定位失败 → `ids.len()`
/// （即全部替换，宁可少保留也不猜位置 —— 猜错会让回放出现无主消息）。
///
/// 对载体公开：写侧维护「消息 ↔ 条目 id」映射时用的是同一条规则
/// （见 `pipi-session` 的 `track_message_ids`），规则只有这一份。
pub fn kept_start(ids: &[String], keep_from_entry: &str) -> usize {
    if keep_from_entry.is_empty() {
        return ids.len();
    }
    ids.iter()
        .position(|id| id == keep_from_entry)
        .unwrap_or(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Api;

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

    fn compaction_entry(id: &str, parent: &str, seq: u64, keep: &str) -> SessionEntry {
        SessionEntry {
            id: id.into(),
            parent_id: Some(parent.into()),
            seq,
            timestamp: seq,
            kind: EntryKind::Compaction {
                summary: "摘要".into(),
                keep_from_entry: keep.into(),
                source_tip: parent.into(),
                strategy: "llm-summarize".into(),
                usage: None,
            },
        }
    }

    #[test]
    fn active_path_walks_parents() {
        let entries = vec![
            message_entry("a", None, 0, "a"),
            message_entry("b", Some("a"), 1, "b"),
            message_entry("c", Some("b"), 2, "c"),
        ];
        let path = active_path(&entries);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].seq, 0);
        assert_eq!(path[2].seq, 2);
    }

    #[test]
    fn replay_keeps_messages_from_first_kept_entry() {
        let entries = vec![
            message_entry("m1", None, 0, "turn 1"),
            message_entry("m2", Some("m1"), 1, "turn 2"),
            message_entry("m3", Some("m2"), 2, "turn 3"),
            message_entry("m4", Some("m3"), 3, "turn 4"),
            compaction_entry("c1", "m4", 4, "m3"),
            message_entry("m5", Some("c1"), 5, "turn 5"),
        ];
        let active = active_path(&entries);
        let (messages, ids) = replay(&active);
        // 摘要 + 保留的 turn 3/4 + 压缩后的 turn 5
        assert_eq!(messages.len(), 4);
        assert_eq!(summary_text(&messages[0]), Some("摘要"));
        let texts: Vec<&str> = messages
            .iter()
            .skip(1)
            .map(|message| match message {
                Message::User { content, .. } => content.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(texts, vec!["turn 3", "turn 4", "turn 5"]);
        // 摘要消息占用压缩条目自身的 id 槽位
        assert_eq!(ids, vec!["c1", "m3", "m4", "m5"]);
    }

    /// 定位不到 keep_from_entry（条目被分叉截断等）时退回「只留摘要」，
    /// 不猜位置、不产生无主消息。
    #[test]
    fn replay_falls_back_to_summary_only_when_kept_entry_is_missing() {
        let entries = vec![
            message_entry("m1", None, 0, "a"),
            compaction_entry("c1", "m1", 1, "不存在的条目 id"),
        ];
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
        let entries = vec![
            message_entry("m1", None, 0, "a"),
            SessionEntry {
                id: "x1".into(),
                parent_id: Some("m1".into()),
                seq: 1,
                timestamp: 1,
                kind: EntryKind::Custom {
                    custom_type: "bookmark".into(),
                },
            },
            SessionEntry {
                id: "c1".into(),
                parent_id: Some("x1".into()),
                seq: 2,
                timestamp: 2,
                kind: EntryKind::ModelChange {
                    provider: Model {
                        id: "m".into(),
                        name: "m".into(),
                        api: Api::AnthropicMessages,
                        base_url: String::new(),
                        max_tokens: 4096,
                        context_window: 100_000,
                    },
                },
            },
        ];
        let messages = rebuild_messages(&active_path(&entries));
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn active_model_reads_latest_model_change() {
        let entries = vec![
            message_entry("m1", None, 0, "a"),
            SessionEntry {
                id: "c1".into(),
                parent_id: Some("m1".into()),
                seq: 1,
                timestamp: 1,
                kind: EntryKind::ModelChange {
                    provider: Model {
                        id: "first".into(),
                        name: "first".into(),
                        api: Api::AnthropicMessages,
                        base_url: String::new(),
                        max_tokens: 4096,
                        context_window: 100_000,
                    },
                },
            },
            SessionEntry {
                id: "c2".into(),
                parent_id: Some("c1".into()),
                seq: 2,
                timestamp: 2,
                kind: EntryKind::ModelChange {
                    provider: Model {
                        id: "second".into(),
                        name: "second".into(),
                        api: Api::OpenAICompletions,
                        base_url: String::new(),
                        max_tokens: 4096,
                        context_window: 100_000,
                    },
                },
            },
        ];
        assert_eq!(
            active_model_from_entries(&entries).map(|model| model.id),
            Some("second".into())
        );
    }

    #[test]
    fn validate_session_id_rejects_paths_and_empty() {
        assert!(validate_session_id("1758000000000-abc123").is_ok());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("../etc/passwd").is_err());
        assert!(validate_session_id("a.b").is_err());
        assert!(validate_session_id("会话").is_err());
    }
}
