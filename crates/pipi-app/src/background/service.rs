use std::collections::VecDeque;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::{Notify, mpsc, oneshot};

use pipi_core::agents;
use pipi_core::tools::background::{
    BackgroundAgentTaskSpec, BackgroundTaskCommand, BackgroundTaskOwner, BackgroundTaskQuery,
    BackgroundTaskService, BackgroundTaskSpec,
};
use pipi_protocol::{
    AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskSnapshot,
    BackgroundTaskStatus, now_millis,
};

use super::agent::{AgentJobEntry, run_agent_job};
use super::persist::{PersistedRecord, append_record, validate_job_id};
use super::shell::{JobEntry, run_job};
use super::state::{JobCommand, JobState, wait_for_state};
use super::{BackgroundTaskManager, CONTROL_TIMEOUT};

#[async_trait]
impl BackgroundTaskService for BackgroundTaskManager {
    async fn submit(&self, spec: BackgroundTaskSpec) -> Result<BackgroundTaskInfo, String> {
        let command = spec.command.trim();
        if command.is_empty() {
            return Err("后台任务 command 不能为空".into());
        }
        let def = agents::load_agent(&spec.owner.agent_name)?;
        let workspace = def
            .resolve_workspace()
            .ok_or_else(|| "无法解析 Agent 工作目录".to_string())?;
        let workspace = std::fs::canonicalize(&workspace)
            .map_err(|error| format!("Agent 工作目录不可用: {error}"))?;
        let cwd = std::fs::canonicalize(&spec.cwd)
            .map_err(|error| format!("后台任务 cwd 不可用: {error}"))?;
        if !cwd.starts_with(&workspace) || !cwd.is_dir() {
            return Err("后台任务 cwd 必须位于 Agent 工作目录内".into());
        }

        let jobs_dir = Self::jobs_dir(&spec.owner.agent_name)?;
        let job_id = format!("job-{}", pipi_agent::session::new_id());
        let metadata_path = jobs_dir.join(format!("{job_id}.jsonl"));
        let log_path = jobs_dir.join(format!("{job_id}.log"));
        let started_at = now_millis();
        let queued = BackgroundTaskInfo {
            job_id: job_id.clone(),
            kind: BackgroundTaskKind::Shell,
            agent_name: spec.owner.agent_name.clone(),
            session_id: spec.owner.session_id.clone(),
            run_id: spec.owner.run_id,
            command: command.to_string(),
            cwd: cwd.display().to_string(),
            target_agent: None,
            child_session_id: None,
            status: BackgroundTaskStatus::Queued,
            started_at,
            completed_at: None,
            exit_code: None,
            output_cursor: 0,
            output_truncated: false,
            error: None,
            result: None,
        };
        self.persist_start(&metadata_path, &queued).await?;

        let mut command_builder = Command::new("bash");
        command_builder
            .arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .env_clear()
            .envs(spec.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // 每个托管任务建立自己的进程组；terminate 会先给整个组 TERM，
        // 再在 grace period 后给整个组 KILL，避免只杀 bash 留下孙进程。
        #[cfg(unix)]
        command_builder.process_group(0);

        let mut child = match command_builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                let mut failed = queued.clone();
                failed.status = BackgroundTaskStatus::Failed;
                failed.completed_at = Some(now_millis());
                failed.error = Some(format!("无法启动后台任务: {error}"));
                append_record(
                    &metadata_path,
                    &PersistedRecord {
                        kind: "failed".into(),
                        task: Some(failed),
                        output: None,
                    },
                )
                .await
                .ok();
                return Err(format!("无法启动后台任务: {error}"));
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let pid = child.id().unwrap_or_default();
        let (control, control_rx) = mpsc::unbounded_channel();
        let info = BackgroundTaskInfo {
            status: BackgroundTaskStatus::Running,
            ..queued
        };
        let entry = Arc::new(JobEntry {
            owner: Mutex::new(spec.owner),
            pid: AtomicU32::new(pid),
            state: Mutex::new(JobState {
                info: info.clone(),
                output: VecDeque::new(),
                output_bytes: 0,
                next_seq: 0,
            }),
            control,
            notify: Notify::new(),
            metadata_path: metadata_path.clone(),
            log_path,
        });
        self.insert_job(entry.clone())?;
        append_record(
            &metadata_path,
            &PersistedRecord {
                kind: "running".into(),
                task: Some(info.clone()),
                output: None,
            },
        )
        .await
        .map_err(|error| {
            eprintln!("pipi: 后台任务运行状态落盘失败: {error}");
            error
        })
        .ok();

        let manager = self.clone();
        self.inner
            .runtime
            .spawn(run_job(manager, entry, child, stdout, stderr, control_rx));
        Ok(info)
    }

    async fn submit_agent(
        &self,
        spec: BackgroundAgentTaskSpec,
    ) -> Result<BackgroundTaskInfo, String> {
        let target_agent = spec.target_agent.trim();
        let prompt = spec.prompt.trim();
        if target_agent.is_empty() {
            return Err("后台 Agent 任务 targetAgent 不能为空".into());
        }
        if target_agent == spec.owner.agent_name {
            return Err("第一阶段不允许 Agent 运行自身".into());
        }
        if prompt.is_empty() {
            return Err("后台 Agent 任务 prompt 不能为空".into());
        }
        if prompt.len() > 64 * 1024 {
            return Err("后台 Agent 任务 prompt 不能超过 65536 字节".into());
        }

        let target = agents::load_agent(target_agent)?;
        let cwd = target
            .resolve_workspace()
            .ok_or_else(|| "无法解析目标 Agent 工作目录".to_string())?;
        let cwd = std::fs::canonicalize(&cwd)
            .map_err(|error| format!("目标 Agent 工作目录不可用: {error}"))?;
        if !cwd.is_dir() {
            return Err("目标 Agent 工作目录不是目录".into());
        }

        let jobs_dir = Self::jobs_dir(&spec.owner.agent_name)?;
        let job_id = format!("job-{}", pipi_agent::session::new_id());
        let metadata_path = jobs_dir.join(format!("{job_id}.jsonl"));
        let started_at = now_millis();
        let queued = BackgroundTaskInfo {
            job_id: job_id.clone(),
            kind: BackgroundTaskKind::Agent,
            agent_name: spec.owner.agent_name.clone(),
            session_id: spec.owner.session_id.clone(),
            run_id: spec.owner.run_id,
            command: format!("run_agent {target_agent}"),
            cwd: cwd.display().to_string(),
            target_agent: Some(target_agent.to_string()),
            child_session_id: None,
            status: BackgroundTaskStatus::Queued,
            started_at,
            completed_at: None,
            exit_code: None,
            output_cursor: 0,
            output_truncated: false,
            error: None,
            result: None,
        };
        self.persist_start(&metadata_path, &queued).await?;

        let abort = AbortSignal::new();
        let info = BackgroundTaskInfo {
            status: BackgroundTaskStatus::Running,
            ..queued
        };
        let session_sink = spec.session_sink.clone();
        let entry = Arc::new(AgentJobEntry {
            owner: Mutex::new(spec.owner),
            abort: abort.clone(),
            state: Mutex::new(JobState {
                info: info.clone(),
                output: VecDeque::new(),
                output_bytes: 0,
                next_seq: 0,
            }),
            notify: Notify::new(),
            metadata_path: metadata_path.clone(),
        });
        self.insert_agent_job(entry.clone())?;
        append_record(
            &metadata_path,
            &PersistedRecord {
                kind: "running".into(),
                task: Some(info.clone()),
                output: None,
            },
        )
        .await
        .map_err(|error| {
            eprintln!("pipi: 后台 Agent 任务运行状态落盘失败: {error}");
            error
        })
        .ok();

        self.inner.runtime.spawn(run_agent_job(
            entry,
            self.inner.child_runner.clone(),
            target_agent.to_string(),
            prompt.to_string(),
            abort,
            session_sink,
        ));
        Ok(info)
    }

    async fn query(
        &self,
        owner: &BackgroundTaskOwner,
        query: BackgroundTaskQuery,
    ) -> Result<Vec<BackgroundTaskSnapshot>, String> {
        if query.limit == 0 || query.limit > 100 {
            return Err("后台任务查询 limit 必须在 1 到 100 之间".into());
        }
        if let Some(job_id) = &query.job_id {
            if let Some(entry) = self.find_job(job_id)? {
                if !entry.belongs_to(owner) {
                    return Err("后台任务不属于当前 Agent session".into());
                }
                return Ok(vec![self.query_live_job(entry, &query).await?]);
            }
            if let Some(entry) = self.find_agent_job(job_id)? {
                if !entry.belongs_to(owner) {
                    return Err("后台任务不属于当前 Agent session".into());
                }
                return Ok(vec![self.query_live_agent_job(entry, &query).await?]);
            }
            return Ok(vec![
                self.query_persisted_job(owner, job_id, query.after_seq)
                    .await?,
            ]);
        }

        let mut snapshots = Vec::new();
        let mut live_ids = std::collections::HashSet::new();
        let entries: Vec<Arc<JobEntry>> = self
            .inner
            .jobs
            .lock()
            .map_err(|error| format!("后台任务表不可用: {error}"))?
            .values()
            .filter(|entry| entry.belongs_to(owner))
            .cloned()
            .collect();
        let agent_entries: Vec<Arc<AgentJobEntry>> = self
            .inner
            .agent_jobs
            .lock()
            .map_err(|error| format!("后台 Agent 任务表不可用: {error}"))?
            .values()
            .filter(|entry| entry.belongs_to(owner))
            .cloned()
            .collect();
        if query.wait_ms > 0
            && entries.iter().all(|entry| !entry.is_active())
            && agent_entries.iter().all(|entry| !entry.is_active())
        {
            tokio::time::sleep(Duration::from_millis(query.wait_ms.min(250))).await;
        }
        for entry in entries {
            live_ids.insert(entry.job_id());
            let snapshot = entry.snapshot(query.after_seq)?;
            if query.include_completed || snapshot.task.status.is_active() {
                snapshots.push(snapshot);
            }
        }
        for entry in agent_entries {
            live_ids.insert(entry.job_id());
            let snapshot = entry.snapshot(query.after_seq)?;
            if query.include_completed || snapshot.task.status.is_active() {
                snapshots.push(snapshot);
            }
        }
        if query.include_completed {
            for snapshot in self.query_persisted_list(owner, query.after_seq).await? {
                if !live_ids.contains(&snapshot.task.job_id) {
                    snapshots.push(snapshot);
                }
            }
        }
        snapshots.sort_by(|left, right| {
            right
                .task
                .started_at
                .cmp(&left.task.started_at)
                .then_with(|| left.task.job_id.cmp(&right.task.job_id))
        });
        snapshots.truncate(query.limit);
        Ok(snapshots)
    }

    async fn manage(
        &self,
        owner: &BackgroundTaskOwner,
        job_id: &str,
        command: BackgroundTaskCommand,
    ) -> Result<BackgroundTaskSnapshot, String> {
        validate_job_id(job_id)?;
        let Some(entry) = self.find_job(job_id)? else {
            if let Some(entry) = self.find_agent_job(job_id)? {
                if !entry.belongs_to(owner) {
                    return Err("后台任务不属于当前 Agent session".into());
                }
                if !entry.is_active() {
                    if matches!(command, BackgroundTaskCommand::WriteStdin(_)) {
                        return Err("后台 Agent 任务已经结束，不能写入 stdin".into());
                    }
                    return entry.snapshot(0);
                }
                if let BackgroundTaskCommand::WriteStdin(_) = command {
                    return Err("后台 Agent 任务不支持写入 stdin".into());
                }
                entry.abort.abort();
                wait_for_state(
                    &entry.state,
                    &entry.notify,
                    CONTROL_TIMEOUT.as_millis() as u64,
                )
                .await;
                return entry.snapshot(0);
            }
            return self.query_persisted_job(owner, job_id, 0).await;
        };
        if !entry.belongs_to(owner) {
            return Err("后台任务不属于当前 Agent session".into());
        }
        if !entry.is_active() {
            if matches!(command, BackgroundTaskCommand::WriteStdin(_)) {
                return Err("后台任务已经结束，不能写入 stdin".into());
            }
            return entry.snapshot(0);
        }

        let (response, receiver) = oneshot::channel();
        let message = match command {
            BackgroundTaskCommand::Terminate => JobCommand::Terminate { response },
            BackgroundTaskCommand::WriteStdin(data) => JobCommand::WriteStdin {
                data: data.into_bytes(),
                response,
            },
        };
        entry
            .control
            .send(message)
            .map_err(|_| "后台任务控制通道已关闭".to_string())?;
        tokio::time::timeout(CONTROL_TIMEOUT, receiver)
            .await
            .map_err(|_| "后台任务控制超时".to_string())?
            .map_err(|_| "后台任务控制通道已断开".to_string())??;
        entry.snapshot(0)
    }

    fn active_count(&self, owner: &BackgroundTaskOwner) -> usize {
        let shell = self
            .inner
            .jobs
            .lock()
            .map(|jobs| {
                jobs.values()
                    .filter(|entry| entry.belongs_to(owner) && entry.is_active())
                    .count()
            })
            .unwrap_or(0);
        let agents = self
            .inner
            .agent_jobs
            .lock()
            .map(|jobs| {
                jobs.values()
                    .filter(|entry| entry.belongs_to(owner) && entry.is_active())
                    .count()
            })
            .unwrap_or(0);
        shell + agents
    }
}
