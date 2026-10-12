//! 会话存储：树状 JSONL 条目，append-only，崩溃安全。
//!
//! 移植自 `packages/agent/src/harness/session/types.ts` 的 Entry 模型
//! （EntryBase / MessageEntry）：`id` + `parentId` + `seq` + `timestamp`，
//! 一行一个 JSON 对象。会话是 Agent 的运行日志 —— 只追加、可分叉
//! （parentId 构成树），Pipi 永远不修改已写入的条目。
//!
//! **格式与载体分离**：本模块的 `SessionEntry` / `EntryKind` / [`replay`] 等是
//! 会话的格式与语义，对所有载体一致；落盘动作走两个接口 —— [`SessionLog`]
//! （追加账本，运行时持有）与 [`SessionRepo`]（目录级操作：建/开/列/分叉/
//! 归档/删）。今天的实现是 JSONL 文件（[`SessionWriter`] / [`JsonlSessionRepo`]）；
//! 换载体只换实现，条目格式与 IPC 契约不变（设计见 `docs/git-session-store.md`）。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use pipi_protocol::{Message, Model, Usage, now_millis};

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

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
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

/// 会话 ID 校验：只允许 ASCII 字母数字与 `-`。
///
/// 会话 id 同时是文件名（JSONL 载体）与引用名（未来的 git 载体），必须拒绝
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

/// 保留区间在 id 序列里的起点下标。空 id 或定位失败 → `ids.len()`
/// （即全部替换，宁可少保留也不猜位置 —— 猜错会让回放出现无主消息）。
fn kept_start(ids: &[String], keep_from_entry: &str) -> usize {
    if keep_from_entry.is_empty() {
        return ids.len();
    }
    ids.iter()
        .position(|id| id == keep_from_entry)
        .unwrap_or(ids.len())
}

/// 压缩条目对 id 序列的作用：摘要消息占用条目自身的 id 槽位，其后跟上
/// 保留区间的 id（见 [`replay`]，两者规则必须一致）。
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

mod history;
mod ledger;
mod repo;

pub use history::{
    SUMMARY_CLOSE_TAG, SUMMARY_OPEN_TAG, active_model_from_entries, active_path, load_session,
    rebuild_messages, replay, summary_message, summary_text,
};
pub use ledger::SessionWriter;
pub use repo::{JsonlSessionRepo, archive_session_file, fork_session, list_session_summaries};
