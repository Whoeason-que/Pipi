//! 应用层后台任务托管。
//!
//! 这里是唯一持有后台子进程的地方。任务按 Agent/session/run 绑定，输出保留一个
//! 有界内存尾部，同时把完整原始日志写到 Agent 的 jobs/ 目录。元数据使用一行
//! 一个 JSON 记录，应用重启后只把最后仍为 live 的记录展示为 orphaned，绝不
//! 根据旧 PID 自动控制进程。
//!
//! 模块划分：`service` 是宿主面向的后台任务服务接口；`shell` 与 `agent` 分别
//! 承载 shell 子进程任务和 child Agent 任务；`state` 提供两类任务共享的状态与
//! 等待原语；`persist` 负责 jobs/ 目录下的日志与 JSONL 元数据。

mod agent;
mod persist;
mod service;
mod shell;
mod state;
#[cfg(test)]
mod test_support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use crate::tools::agent::{AgentRunResult, ChildProgressTx};
use crate::tools::background::{
    BackgroundAgentSessionSink, BackgroundTaskOwner, BackgroundTaskQuery,
};
use pipi_protocol::{
    AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskResult,
    BackgroundTaskSnapshot,
};
use pipi_venv::agents;

use self::agent::AgentJobEntry;

/// 取文本末尾的不超过 `max_bytes` 的部分，按字符边界与整行对齐。
fn tail_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.trim_end().to_string();
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    // 从第一个换行之后开始，避免把第一行截成半截。
    let tail = tail.find('\n').map_or(tail, |at| &tail[at + 1..]);
    format!("[…output truncated…]\n{}", tail.trim_end())
}

use self::persist::{PersistedRecord, append_record, load_persisted_snapshot, validate_job_id};
use self::shell::JobEntry;
use self::state::{JobCore, wait_for_state};

pub(super) const MAX_OUTPUT_BYTES: usize = 512 * 1024;
pub(super) const TERMINATE_GRACE: Duration = Duration::from_secs(2);
pub(super) const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// 运行 child Agent 的能力。由 runtime 注入（`runtime/execution.rs`），
/// background 只编排任务状态与输出，不反向依赖 runtime。
#[async_trait]
pub trait ChildAgentRunner: Send + Sync {
    async fn run_child(
        &self,
        target_agent: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
        session_sink: Option<BackgroundAgentSessionSink>,
    ) -> Result<AgentRunResult, String>;
}

#[derive(Clone)]
pub struct BackgroundTaskManager {
    inner: Arc<Inner>,
}

struct Inner {
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) child_runner: Arc<dyn ChildAgentRunner>,
    /// 全部托管任务（shell 子进程与 child Agent）一张表：job_id 全局唯一，
    /// 后端差异只在需要处 match [`JobHandle`]。
    jobs: Mutex<HashMap<String, JobHandle>>,
    accepting: tokio::sync::RwLock<bool>,
    /// 终态广播：宿主的唤醒器订阅它决定要不要把结果写回会话；UI 与推送服务
    /// 也可以订阅同一份事件。没有订阅者时发送是空操作。
    completion: tokio::sync::broadcast::Sender<BackgroundTaskInfo>,
}

/// 表里的一条任务。绝大部分操作（身份、状态、快照、输出）走共用的
/// [`JobCore`]；两种后端仅在终止方式上不同（控制通道 vs abort）。
#[derive(Clone)]
enum JobHandle {
    Shell(Arc<JobEntry>),
    Agent(Arc<AgentJobEntry>),
}

impl JobHandle {
    fn core(&self) -> &JobCore {
        match self {
            Self::Shell(entry) => &entry.core,
            Self::Agent(entry) => &entry.core,
        }
    }

    fn job_id(&self) -> String {
        self.core().job_id()
    }

    fn belongs_to(&self, owner: &BackgroundTaskOwner) -> bool {
        self.core().belongs_to(owner)
    }

    fn is_active(&self) -> bool {
        self.core().is_active()
    }

    fn snapshot(&self, after_seq: u64) -> Result<BackgroundTaskSnapshot, String> {
        self.core().snapshot(after_seq)
    }
}

impl BackgroundTaskManager {
    pub fn new(runtime: tokio::runtime::Handle, child_runner: Arc<dyn ChildAgentRunner>) -> Self {
        // 容量只影响滞后时丢事件的多少；终态是低频事件，订阅者背压不做强保证。
        let (completion, _) = tokio::sync::broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                runtime,
                child_runner,
                jobs: Mutex::new(HashMap::new()),
                accepting: tokio::sync::RwLock::new(true),
                completion,
            }),
        }
    }

    /// 订阅后台任务的终态（Completed / Failed / Terminated）。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BackgroundTaskInfo> {
        self.inner.completion.subscribe()
    }

    /// 某任务是否已经被一次 `query`（含 waitMs）读到过终态。
    pub fn was_observed(&self, job_id: &str) -> bool {
        self.find_job(job_id)
            .ok()
            .flatten()
            .is_some_and(|handle| handle.core().was_observed())
    }

    /// Reconcile missed completion broadcasts after a slow subscriber falls behind.
    pub fn completion_candidates(&self) -> Vec<BackgroundTaskInfo> {
        self.inner
            .jobs
            .lock()
            .map(|jobs| jobs.values().map(|job| job.core().info()).collect())
            .unwrap_or_default()
    }

    /// 把一条终态信息整理成唤醒通知。
    ///
    /// 输出只保留尾部（`max_output_bytes` 内的完整行），完整日志仍在
    /// jobs/*.log 里；child Agent 用最终文本而不是过程输出。
    pub fn completion_result(
        &self,
        info: &BackgroundTaskInfo,
        max_output_bytes: usize,
    ) -> BackgroundTaskResult {
        let mut output = String::new();
        if info.kind == BackgroundTaskKind::Agent {
            if let Some(result) = &info.result {
                output = tail_text(result, max_output_bytes);
            }
        } else if let Ok(Some(handle)) = self.find_job(&info.job_id)
            && let Ok(snapshot) = handle.snapshot(0)
        {
            let text: String = snapshot
                .output
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect();
            output = tail_text(&text, max_output_bytes);
        }
        BackgroundTaskResult {
            job_id: info.job_id.clone(),
            kind: info.kind,
            status: info.status,
            command: info.command.clone(),
            target_agent: info.target_agent.clone(),
            exit_code: info.exit_code,
            error: info.error.clone(),
            output,
        }
    }

    /// 停止接收任务，并等待 shell / child Agent 完成正常收尾。
    pub async fn shutdown(&self) -> Result<(), String> {
        // 写锁会等正在提交的任务完成登记；之后快照不会漏掉启动中的任务。
        *self.inner.accepting.write().await = false;
        let jobs: Vec<_> = self
            .inner
            .jobs
            .lock()
            .map_err(|error| format!("Background task table is unavailable: {error}"))?
            .values()
            .cloned()
            .collect();
        for job in &jobs {
            if job.is_active() {
                match job {
                    JobHandle::Agent(entry) => entry.abort.abort(),
                    JobHandle::Shell(entry) => {
                        let (response, _) = tokio::sync::oneshot::channel();
                        let _ = entry
                            .control
                            .send(state::JobCommand::Terminate { response });
                    }
                }
            }
        }
        while jobs.iter().any(|job| {
            !job.core()
                .finished
                .load(std::sync::atomic::Ordering::Acquire)
        }) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    /// 压缩分叉把逻辑 session 换成新 id 时，后台任务仍属于同一条逻辑会话。
    /// 同步更新内存 owner 与任务元数据，并异步追加一条新 owner 记录，保证重启
    /// 后仍能从新 session 查询到尚未结束的任务。
    pub fn rekey_session(&self, agent_name: &str, old_session_id: &str, new_session_id: &str) {
        if old_session_id == new_session_id {
            return;
        }
        let mut updates = Vec::new();
        if let Ok(jobs) = self.inner.jobs.lock() {
            for handle in jobs.values() {
                let core = handle.core();
                if let Some(info) = core.rekey(agent_name, old_session_id, new_session_id) {
                    updates.push((core.metadata_path.clone(), info));
                }
            }
        }
        for (path, task) in updates {
            let runtime = self.inner.runtime.clone();
            runtime.spawn(async move {
                let _ = append_record(
                    &path,
                    &PersistedRecord {
                        kind: "rekeyed".into(),
                        task: Some(task),
                        output: None,
                    },
                )
                .await;
            });
        }
    }

    fn jobs_dir(agent_name: &str) -> Result<PathBuf, String> {
        agents::validate_agent_name(agent_name)?;
        let agent_dir = agents::agent_dir(agent_name).ok_or_else(|| {
            "Cannot locate user home directory; cannot create background task directory".to_string()
        })?;
        if !pipi_protocol::fs::ensure_real_directory(&agent_dir, "Agent directory")? {
            return Err(format!("Agent \"{agent_name}\" does not exist"));
        }
        let jobs_dir = agent_dir.join("jobs");
        match pipi_protocol::fs::ensure_real_directory(&jobs_dir, "Background task directory")? {
            true => {}
            false => {
                std::fs::create_dir_all(&jobs_dir)
                    .map_err(|error| format!("Cannot create background task directory: {error}"))?;
                pipi_protocol::fs::ensure_real_directory(&jobs_dir, "Background task directory")?;
            }
        }
        Ok(jobs_dir)
    }

    fn insert_job(&self, handle: JobHandle) -> Result<(), String> {
        let job_id = handle.job_id();
        self.inner
            .jobs
            .lock()
            .map_err(|error| format!("Background task table is unavailable: {error}"))?
            .insert(job_id, handle);
        Ok(())
    }

    fn find_job(&self, job_id: &str) -> Result<Option<JobHandle>, String> {
        Ok(self
            .inner
            .jobs
            .lock()
            .map_err(|error| format!("Background task table is unavailable: {error}"))?
            .get(job_id)
            .cloned())
    }

    async fn query_live_job(
        &self,
        handle: JobHandle,
        query: &BackgroundTaskQuery,
    ) -> Result<BackgroundTaskSnapshot, String> {
        if query.wait_ms > 0 {
            let core = handle.core();
            wait_for_state(&core.state, &core.notify, query.wait_ms).await;
        }
        let snapshot = handle.snapshot(query.after_seq)?;
        // 模型已经通过查询看到终态：唤醒器不再重复推同一条结果。
        if query.observe && !snapshot.task.status.is_active() {
            handle.core().mark_observed();
        }
        Ok(snapshot)
    }

    async fn query_persisted_job(
        &self,
        owner: &BackgroundTaskOwner,
        job_id: &str,
        after_seq: u64,
    ) -> Result<BackgroundTaskSnapshot, String> {
        validate_job_id(job_id)?;
        let path = Self::jobs_dir(&owner.agent_name)?.join(format!("{job_id}.jsonl"));
        let Some(snapshot) = load_persisted_snapshot(&path, owner, after_seq).await? else {
            return Err(format!(
                "Background task \"{job_id}\" does not exist or does not belong to current session"
            ));
        };
        Ok(snapshot)
    }

    async fn query_persisted_list(
        &self,
        owner: &BackgroundTaskOwner,
        after_seq: u64,
    ) -> Result<Vec<BackgroundTaskSnapshot>, String> {
        let jobs_dir = Self::jobs_dir(&owner.agent_name)?;
        let mut dir = tokio::fs::read_dir(&jobs_dir)
            .await
            .map_err(|error| format!("Cannot read background task directory: {error}"))?;
        let mut snapshots = Vec::new();
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|error| format!("Cannot enumerate background task directory: {error}"))?
        {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(snapshot) = load_persisted_snapshot(&path, owner, after_seq).await? {
                snapshots.push(snapshot);
            }
        }
        Ok(snapshots)
    }

    async fn persist_start(&self, path: &Path, task: &BackgroundTaskInfo) -> Result<(), String> {
        append_record(
            path,
            &PersistedRecord {
                kind: "started".into(),
                task: Some(task.clone()),
                output: None,
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::tail_text;

    #[test]
    fn tail_text_keeps_whole_lines_and_respects_char_boundaries() {
        // 未超限：原样（去掉尾部空白）。
        assert_eq!(tail_text("one\ntwo\n", 100), "one\ntwo");

        // 超限：从完整行开始，带截断标记，且不含被截掉的开头。
        let tail = tail_text("first\nsecond\nthird", 12);
        assert!(tail.starts_with("[…output truncated…]"), "{tail}");
        assert!(tail.ends_with("third"), "{tail}");
        assert!(!tail.contains("first"), "{tail}");

        // 多字节字符不能被切半：起点落在字符中间时向后对齐。
        let tail = tail_text("一二三", 4);
        assert!(tail.ends_with("三"), "{tail}");
    }
}
