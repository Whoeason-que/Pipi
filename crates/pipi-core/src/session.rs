//! 会话存储：树状 JSONL 条目，append-only，崩溃安全。
//!
//! 移植自 `packages/agent/src/harness/session/types.ts` 的 Entry 模型
//! （EntryBase / MessageEntry）：`id` + `parentId` + `seq` + `timestamp`，
//! 一行一个 JSON 对象。会话是 Agent 的运行日志 —— 只追加、可分叉
//! （parentId 构成树），Pipi 永远不修改已写入的条目。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{now_millis, Message, Model, Usage};

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
    // 活跃路径上已有环境记账：置位，避免分叉出的会话再记一条（append_env 幂等）
    if active
        .iter()
        .any(|entry| matches!(entry.kind, EntryKind::Env { .. }))
    {
        writer.mark_env_recorded();
    }
    Ok(writer)
}

/// 把会话文件移入归档区（`sessions/.archive/<id>.jsonl`），返回新路径。
///
/// 纯移动、无标记（文件即真相，见 `agents::ARCHIVE_DIR`）。自由函数形式是为了
/// 让运行任务（压缩换会话后归档原文件）也能调用 —— `RuntimeState::archive_session`
/// 在其上加了「会话正打开时拒绝」的占用检查。
pub fn archive_session_file(sessions_dir: &Path, session_id: &str) -> Result<PathBuf, String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("非法会话 ID".into());
    }
    let source = sessions_dir.join(format!("{session_id}.jsonl"));
    if !crate::agents::ensure_real_file(&source, "会话文件")? {
        return Err("会话不存在".into());
    }
    let archive_dir = sessions_dir.join(crate::agents::ARCHIVE_DIR);
    // 归档目录：不存在则创建；存在必须是真实目录（防符号链接）
    if !crate::agents::ensure_real_directory(&archive_dir, "归档目录")? {
        std::fs::create_dir_all(&archive_dir).map_err(|e| format!("无法创建归档目录: {e}"))?;
    }
    let target = archive_dir.join(format!("{session_id}.jsonl"));
    if target.exists() {
        return Err("归档区已存在同名会话".into());
    }
    std::fs::rename(&source, &target).map_err(|e| format!("归档会话失败: {e}"))?;
    Ok(target)
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
    use crate::types::ContentBlock;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pipi-test-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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

        let entries = load_session(writer.path()).unwrap();
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
    fn replay_keeps_messages_from_first_kept_entry() {
        let dir = temp_dir();
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
        let dir = temp_dir();
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
    fn compaction_entry_persists_summary_usage() {
        let dir = temp_dir();
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
    fn rebuild_without_compaction_keeps_all_messages() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("a")).unwrap();
        writer.append_custom("bookmark").unwrap();
        writer
            .append_model_change(&Model {
                id: "m".into(),
                name: "m".into(),
                api: crate::types::Api::AnthropicMessages,
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

    #[test]
    fn archive_session_file_moves_and_refuses_conflicts() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("hi")).unwrap();
        let path = writer.path().to_path_buf();
        let id = path.file_stem().unwrap().to_str().unwrap().to_string();
        drop(writer);

        let archived = archive_session_file(&dir, &id).unwrap();
        assert!(!path.exists(), "原文件应当已经移走");
        assert!(archived.is_file(), "归档区应当有该文件");
        assert_eq!(
            archived.file_name().unwrap().to_str().unwrap(),
            format!("{id}.jsonl")
        );
        // 归档后活跃区查不到
        assert!(!dir.join(format!("{id}.jsonl")).exists());

        // 重复归档：活跃区已无此会话
        assert!(archive_session_file(&dir, &id).is_err());
        // 非法 id
        assert!(archive_session_file(&dir, "../evil").is_err());
        // 目标已存在时拒绝（把文件放回去再归档一次）
        std::fs::rename(&archived, &path).unwrap();
        std::fs::write(&archived, "{}\n").unwrap();
        let error = archive_session_file(&dir, &id).unwrap_err();
        assert!(error.contains("已存在"), "{error}");
    }

    #[test]
    fn fork_marks_env_recorded_when_path_already_has_env_entry() {
        let dir = temp_dir();
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("hi")).unwrap();
        writer
            .append_env(vec![EnvDeclared {
                key: "AV_AGENT".into(),
                source: "runtime".into(),
            }])
            .unwrap();
        let source_path = writer.path().to_path_buf();
        drop(writer);

        let mut forked = fork_session(&source_path, &dir, None).unwrap();
        // 活跃路径已带 Env 条目 → 分叉出的会话不应再记一条
        assert!(forked
            .append_env(vec![EnvDeclared {
                key: "AV_AGENT".into(),
                source: "runtime".into(),
            }])
            .unwrap()
            .is_none());
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
        let id3 = forked
            .append_message(&Message::user_text("msg 3 branch"))
            .unwrap();
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
