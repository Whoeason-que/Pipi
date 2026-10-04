use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use pipi_core::tools::background::BackgroundTaskOwner;
use pipi_protocol::{
    BackgroundTaskInfo, BackgroundTaskOutput, BackgroundTaskSnapshot, BackgroundTaskStatus,
    now_millis,
};

use super::MAX_OUTPUT_BYTES;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PersistedRecord {
    pub(super) kind: String,
    #[serde(default)]
    pub(super) task: Option<BackgroundTaskInfo>,
    #[serde(default)]
    pub(super) output: Option<BackgroundTaskOutput>,
}

pub(super) async fn append_log(path: &Path, text: &str) -> Result<(), String> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| format!("打开后台任务日志失败: {error}"))?;
    file.write_all(text.as_bytes())
        .await
        .map_err(|error| format!("写入后台任务日志失败: {error}"))
}

pub(super) async fn append_record(path: &Path, record: &PersistedRecord) -> Result<(), String> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| format!("打开后台任务状态文件失败: {error}"))?;
    let line = serde_json::to_string(record)
        .map_err(|error| format!("序列化后台任务状态失败: {error}"))?;
    file.write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|error| format!("写入后台任务状态失败: {error}"))
}

pub(super) async fn load_persisted_snapshot(
    path: &Path,
    owner: &BackgroundTaskOwner,
    after_seq: u64,
) -> Result<Option<BackgroundTaskSnapshot>, String> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取后台任务状态失败: {error}")),
    };
    let mut last = None;
    let mut output = VecDeque::new();
    let mut output_bytes = 0usize;
    let mut output_truncated = false;
    for line in content.lines() {
        if let Ok(record) = serde_json::from_str::<PersistedRecord>(line) {
            if let Some(task) = record.task {
                last = Some(task);
            }
            if let Some(chunk) = record.output {
                output_bytes = output_bytes.saturating_add(chunk.text.len());
                output.push_back(chunk);
                while output_bytes > MAX_OUTPUT_BYTES {
                    let Some(old) = output.pop_front() else {
                        break;
                    };
                    output_bytes = output_bytes.saturating_sub(old.text.len());
                    output_truncated = true;
                }
            }
        }
    }
    let Some(mut task) = last else {
        return Ok(None);
    };
    if task.agent_name != owner.agent_name || task.session_id != owner.session_id {
        return Ok(None);
    }
    if task.status.is_active() {
        task.status = BackgroundTaskStatus::Orphaned;
        task.completed_at = Some(now_millis());
        task.error = Some("Pipi 重启前任务仍处于运行态；为避免 PID 复用，未自动接管".into());
    }
    task.output_truncated |= output_truncated;
    task.output_cursor = task
        .output_cursor
        .max(output.back().map(|chunk| chunk.seq).unwrap_or_default());
    Ok(Some(BackgroundTaskSnapshot {
        next_seq: task.output_cursor,
        task,
        output: output
            .into_iter()
            .filter(|chunk| chunk.seq > after_seq)
            .collect(),
    }))
}

pub(super) fn validate_job_id(job_id: &str) -> Result<(), String> {
    if job_id.is_empty()
        || !job_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err("非法后台任务 ID".into());
    }
    Ok(())
}
