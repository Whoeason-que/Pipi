use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::tools::background::BackgroundTaskOwner;
use pipi_protocol::{
    BackgroundTaskInfo, BackgroundTaskOutput, BackgroundTaskSnapshot, BackgroundTaskStatus,
    now_millis,
};

use super::MAX_OUTPUT_BYTES;
use super::state::JobCore;

/// 把一条任务输出追加到内存快照并落盘元数据；空输出由 `append_output` 忽略。
/// 写盘失败保持现有「只忽略、不影响任务」语义。
pub(super) async fn record_output(core: &JobCore, stream: &'static str, text: String) {
    let Ok(Some(output)) = core.append_output(stream, text) else {
        return;
    };
    let _ = append_record(
        &core.metadata_path,
        &PersistedRecord {
            kind: "output".into(),
            task: None,
            output: Some(output),
        },
    )
    .await;
}

/// 状态 JSONL 的进程内单写者：rekey、output、finished 可能来自不同任务，
/// 不能让它们的 `write_all` 交错成半行。
static STATE_WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        .map_err(|error| format!("Failed to open background task log: {error}"))?;
    file.write_all(text.as_bytes())
        .await
        .map_err(|error| format!("Failed to write background task log: {error}"))
}

pub(super) async fn append_record(path: &Path, record: &PersistedRecord) -> Result<(), String> {
    let _state_guard = STATE_WRITE_LOCK.lock().await;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| format!("Failed to open background task state file: {error}"))?;
    let line = serde_json::to_string(record)
        .map_err(|error| format!("Failed to serialize background task state: {error}"))?;
    file.write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|error| format!("Failed to write background task state: {error}"))
}

pub(super) async fn load_persisted_snapshot(
    path: &Path,
    owner: &BackgroundTaskOwner,
    after_seq: u64,
) -> Result<Option<BackgroundTaskSnapshot>, String> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to read background task state: {error}")),
    };
    let mut lines = BufReader::new(file).lines();
    let mut last = None;
    let mut output = VecDeque::new();
    let mut output_bytes = 0usize;
    let mut output_truncated = false;
    // 流式扫描：长任务的状态文件可能很大，只保留最后一条任务元数据与末尾
    // 输出窗口，不能先 read_to_string 把整个文件载入内存。
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read background task state: {error}"))?
    {
        if let Ok(record) = serde_json::from_str::<PersistedRecord>(&line) {
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
        task.error = Some("Task was still running before Pipi restarted; not automatically taken over to prevent PID reuse".into());
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
        return Err("Invalid background task ID".into());
    }
    Ok(())
}
