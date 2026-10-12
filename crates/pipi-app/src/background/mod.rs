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

use pipi_core::tools::agent::{AgentRunResult, ChildProgressTx};
use pipi_core::tools::background::{
    BackgroundAgentSessionSink, BackgroundTaskOwner, BackgroundTaskQuery,
};
use pipi_protocol::{AbortSignal, BackgroundTaskInfo, BackgroundTaskSnapshot};
use pipi_venv::agents;

use self::agent::AgentJobEntry;
use self::persist::{PersistedRecord, append_record, load_persisted_snapshot, validate_job_id};
use self::shell::JobEntry;
use self::state::{JobState, wait_for_state};

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
    pub(super) jobs: Mutex<HashMap<String, Arc<JobEntry>>>,
    pub(super) agent_jobs: Mutex<HashMap<String, Arc<AgentJobEntry>>>,
}

impl BackgroundTaskManager {
    pub fn new(runtime: tokio::runtime::Handle, child_runner: Arc<dyn ChildAgentRunner>) -> Self {
        Self {
            inner: Arc::new(Inner {
                runtime,
                child_runner,
                jobs: Mutex::new(HashMap::new()),
                agent_jobs: Mutex::new(HashMap::new()),
            }),
        }
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
            for entry in jobs.values() {
                if rekey_job_owner(
                    &entry.owner,
                    &entry.state,
                    agent_name,
                    old_session_id,
                    new_session_id,
                ) {
                    updates.push((entry.metadata_path.clone(), entry.info()));
                }
            }
        }
        if let Ok(jobs) = self.inner.agent_jobs.lock() {
            for entry in jobs.values() {
                if rekey_job_owner(
                    &entry.owner,
                    &entry.state,
                    agent_name,
                    old_session_id,
                    new_session_id,
                ) {
                    updates.push((entry.metadata_path.clone(), entry.info()));
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
        let agent_dir = agents::agent_dir(agent_name)
            .ok_or_else(|| "无法定位用户主目录，不能创建后台任务目录".to_string())?;
        if !pipi_protocol::fs::ensure_real_directory(&agent_dir, "Agent 目录")? {
            return Err(format!("Agent「{agent_name}」不存在"));
        }
        let jobs_dir = agent_dir.join("jobs");
        match pipi_protocol::fs::ensure_real_directory(&jobs_dir, "后台任务目录")? {
            true => {}
            false => {
                std::fs::create_dir_all(&jobs_dir)
                    .map_err(|error| format!("无法创建后台任务目录: {error}"))?;
                pipi_protocol::fs::ensure_real_directory(&jobs_dir, "后台任务目录")?;
            }
        }
        Ok(jobs_dir)
    }

    fn insert_job(&self, entry: Arc<JobEntry>) -> Result<(), String> {
        let job_id = entry.info().job_id;
        self.inner
            .jobs
            .lock()
            .map_err(|error| format!("后台任务表不可用: {error}"))?
            .insert(job_id, entry);
        Ok(())
    }

    fn find_job(&self, job_id: &str) -> Result<Option<Arc<JobEntry>>, String> {
        Ok(self
            .inner
            .jobs
            .lock()
            .map_err(|error| format!("后台任务表不可用: {error}"))?
            .get(job_id)
            .cloned())
    }

    fn insert_agent_job(&self, entry: Arc<AgentJobEntry>) -> Result<(), String> {
        let job_id = entry.info().job_id;
        self.inner
            .agent_jobs
            .lock()
            .map_err(|error| format!("后台 Agent 任务表不可用: {error}"))?
            .insert(job_id, entry);
        Ok(())
    }

    fn find_agent_job(&self, job_id: &str) -> Result<Option<Arc<AgentJobEntry>>, String> {
        Ok(self
            .inner
            .agent_jobs
            .lock()
            .map_err(|error| format!("后台 Agent 任务表不可用: {error}"))?
            .get(job_id)
            .cloned())
    }

    async fn query_live_job(
        &self,
        entry: Arc<JobEntry>,
        query: &BackgroundTaskQuery,
    ) -> Result<BackgroundTaskSnapshot, String> {
        if query.wait_ms > 0 {
            wait_for_state(&entry.state, &entry.notify, query.wait_ms).await;
        }
        entry.snapshot(query.after_seq)
    }

    async fn query_live_agent_job(
        &self,
        entry: Arc<AgentJobEntry>,
        query: &BackgroundTaskQuery,
    ) -> Result<BackgroundTaskSnapshot, String> {
        if query.wait_ms > 0 {
            wait_for_state(&entry.state, &entry.notify, query.wait_ms).await;
        }
        entry.snapshot(query.after_seq)
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
            return Err(format!("后台任务「{job_id}」不存在或不属于当前 session"));
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
            .map_err(|error| format!("无法读取后台任务目录: {error}"))?;
        let mut snapshots = Vec::new();
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|error| format!("无法枚举后台任务目录: {error}"))?
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

fn rekey_job_owner(
    owner: &Mutex<BackgroundTaskOwner>,
    state: &Mutex<JobState>,
    agent_name: &str,
    old_session_id: &str,
    new_session_id: &str,
) -> bool {
    let Ok(mut current) = owner.lock() else {
        return false;
    };
    if current.agent_name != agent_name || current.session_id != old_session_id {
        return false;
    }
    current.session_id = new_session_id.to_string();
    drop(current);
    let Ok(mut state) = state.lock() else {
        return false;
    };
    state.info.session_id = new_session_id.to_string();
    true
}
