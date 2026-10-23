use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::tools::background::{
    BackgroundAgentTaskSpec, BackgroundTaskCommand, BackgroundTaskOwner, BackgroundTaskQuery,
    BackgroundTaskService, BackgroundTaskSpec,
};
use pipi_protocol::{
    AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskSnapshot,
    BackgroundTaskStatus, now_millis,
};
use pipi_venv::agents;

use super::agent::{AgentJobEntry, run_agent_job};
use super::persist::{PersistedRecord, append_record, validate_job_id};
use super::shell::{JobEntry, run_job};
use super::state::{JobCommand, JobCore, wait_for_state};
use super::{BackgroundTaskManager, CONTROL_TIMEOUT, JobHandle};

#[async_trait]
impl BackgroundTaskService for BackgroundTaskManager {
    async fn submit(&self, spec: BackgroundTaskSpec) -> Result<BackgroundTaskInfo, String> {
        let command = spec.command.trim();
        if command.is_empty() {
            return Err("Background task command cannot be empty".into());
        }
        let def = agents::load_agent(&spec.owner.agent_name)?;
        let workspace = def
            .resolve_workspace()
            .ok_or_else(|| "Cannot resolve Agent working directory".to_string())?;
        let workspace = std::fs::canonicalize(&workspace)
            .map_err(|error| format!("Agent working directory is not available: {error}"))?;
        let cwd = std::fs::canonicalize(&spec.cwd)
            .map_err(|error| format!("Background task cwd is not available: {error}"))?;
        if !cwd.starts_with(&workspace) || !cwd.is_dir() {
            return Err("Background task cwd must be located within Agent working directory".into());
        }

        let jobs_dir = Self::jobs_dir(&spec.owner.agent_name)?;
        let job_id = format!("job-{}", pipi_protocol::new_id());
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
                failed.error = Some(format!("Failed to start background task: {error}"));
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
                return Err(format!("Failed to start background task: {error}"));
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
            core: JobCore::new(
                BackgroundTaskKind::Shell,
                spec.owner,
                info.clone(),
                metadata_path.clone(),
            ),
            pid: AtomicU32::new(pid),
            control,
            log_path,
        });
        self.insert_job(JobHandle::Shell(entry.clone()))?;
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
            eprintln!("pipi: failed to persist background task running state: {error}");
            error
        })
        .ok();

        self.inner
            .runtime
            .spawn(run_job(entry, child, stdout, stderr, control_rx));
        Ok(info)
    }

    async fn submit_agent(
        &self,
        spec: BackgroundAgentTaskSpec,
    ) -> Result<BackgroundTaskInfo, String> {
        let target_agent = spec.target_agent.trim();
        let prompt = spec.prompt.trim();
        if target_agent.is_empty() {
            return Err("Background Agent task targetAgent cannot be empty".into());
        }
        if target_agent == spec.owner.agent_name {
            return Err("Agent is not allowed to run itself".into());
        }
        if prompt.is_empty() {
            return Err("Background Agent task prompt cannot be empty".into());
        }
        if prompt.len() > 64 * 1024 {
            return Err("Background Agent task prompt cannot exceed 65536 bytes".into());
        }

        let target = agents::load_agent(target_agent)?;
        let cwd = target
            .resolve_workspace()
            .ok_or_else(|| "Cannot resolve target Agent working directory".to_string())?;
        let cwd = std::fs::canonicalize(&cwd)
            .map_err(|error| format!("Target Agent working directory is not available: {error}"))?;
        if !cwd.is_dir() {
            return Err("Target Agent working directory is not a directory".into());
        }

        let jobs_dir = Self::jobs_dir(&spec.owner.agent_name)?;
        let job_id = format!("job-{}", pipi_protocol::new_id());
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
            core: JobCore::new(
                BackgroundTaskKind::Agent,
                spec.owner,
                info.clone(),
                metadata_path.clone(),
            ),
            abort: abort.clone(),
        });
        self.insert_job(JobHandle::Agent(entry.clone()))?;
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
            eprintln!("pipi: failed to persist background agent task running state: {error}");
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
            return Err("Background task query limit must be between 1 and 100".into());
        }
        if let Some(job_id) = &query.job_id {
            if let Some(handle) = self.find_job(job_id)? {
                if !handle.belongs_to(owner) {
                    return Err("Background task does not belong to current Agent session".into());
                }
                return Ok(vec![self.query_live_job(handle, &query).await?]);
            }
            return Ok(vec![
                self.query_persisted_job(owner, job_id, query.after_seq)
                    .await?,
            ]);
        }

        let mut snapshots = Vec::new();
        let mut live_ids = std::collections::HashSet::new();
        let handles: Vec<JobHandle> = self
            .inner
            .jobs
            .lock()
            .map_err(|error| format!("Background task table is unavailable: {error}"))?
            .values()
            .filter(|handle| handle.belongs_to(owner))
            .cloned()
            .collect();
        if query.wait_ms > 0 && handles.iter().all(|handle| !handle.is_active()) {
            tokio::time::sleep(Duration::from_millis(query.wait_ms.min(250))).await;
        }
        for handle in handles {
            live_ids.insert(handle.job_id());
            let snapshot = handle.snapshot(query.after_seq)?;
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
        let Some(handle) = self.find_job(job_id)? else {
            return self.query_persisted_job(owner, job_id, 0).await;
        };
        if !handle.belongs_to(owner) {
            return Err("Background task does not belong to current Agent session".into());
        }
        let label = handle.core().label();
        if !handle.is_active() {
            if matches!(command, BackgroundTaskCommand::WriteStdin(_)) {
                return Err(format!("{label} has ended, cannot write to stdin"));
            }
            return handle.snapshot(0);
        }

        match &handle {
            // shell 任务：经控制通道写 stdin / 终止
            JobHandle::Shell(entry) => {
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
                    .map_err(|_| "Background task control channel is closed".to_string())?;
                tokio::time::timeout(CONTROL_TIMEOUT, receiver)
                    .await
                    .map_err(|_| "Background task control timed out".to_string())?
                    .map_err(|_| "Background task control channel disconnected".to_string())??;
                handle.snapshot(0)
            }
            // child Agent 任务：只支持终止（abort），没有 stdin
            JobHandle::Agent(entry) => {
                if let BackgroundTaskCommand::WriteStdin(_) = command {
                    return Err("Background Agent task does not support writing to stdin".into());
                }
                entry.abort.abort();
                let core = &entry.core;
                wait_for_state(
                    &core.state,
                    &core.notify,
                    CONTROL_TIMEOUT.as_millis() as u64,
                )
                .await;
                handle.snapshot(0)
            }
        }
    }

    fn active_count(&self, owner: &BackgroundTaskOwner) -> usize {
        self.inner
            .jobs
            .lock()
            .map(|jobs| {
                jobs.values()
                    .filter(|handle| handle.belongs_to(owner) && handle.is_active())
                    .count()
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn submit_query_manage_and_restart_are_session_scoped() {
        let _home_guard = pipi_test_support::HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pipi-background-{}-{}",
            std::process::id(),
            pipi_protocol::new_id()
        ));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        pipi_test_support::set_env_var("HOME", &home);
        let permissions = pipi_protocol::permissions::PermissionsConfig {
            tools: pipi_protocol::permissions::BACKGROUND_TASK_TOOLS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            ..Default::default()
        };
        pipi_venv::agents::create_agent(
            "background-test",
            "background test",
            Some(workspace.to_str().unwrap()),
            Some(permissions),
            None,
            None,
        )
        .unwrap();

        let manager = BackgroundTaskManager::new(
            tokio::runtime::Handle::current(),
            Arc::new(crate::runtime::RuntimeChildRunner),
        );
        let task_owner = BackgroundTaskOwner::new("background-test", "session-a", 1);
        let info = manager
            .submit(BackgroundTaskSpec {
                owner: task_owner.clone(),
                command: "printf first; sleep 0.1; printf second".into(),
                cwd: workspace.clone(),
                env: std::collections::BTreeMap::new(),
            })
            .await
            .unwrap();
        let mut snapshot = None;
        for _ in 0..20 {
            let current = manager
                .query(
                    &task_owner,
                    BackgroundTaskQuery {
                        job_id: Some(info.job_id.clone()),
                        wait_ms: 100,
                        include_completed: true,
                        limit: 20,
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
                .pop()
                .unwrap();
            if !current.task.status.is_active() {
                snapshot = Some(current);
                break;
            }
        }
        let snapshot = snapshot.expect("后台任务应在测试窗口内完成");
        assert_eq!(snapshot.task.status, BackgroundTaskStatus::Completed);
        assert!(
            snapshot
                .output
                .iter()
                .any(|chunk| chunk.text.contains("first"))
        );
        let cursor = snapshot.next_seq;
        let incremental = manager
            .query(
                &BackgroundTaskOwner::new("background-test", "session-a", 99),
                BackgroundTaskQuery {
                    job_id: Some(info.job_id.clone()),
                    after_seq: cursor,
                    include_completed: true,
                    limit: 20,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(incremental[0].output.is_empty());
        let reopened_manager = BackgroundTaskManager::new(
            tokio::runtime::Handle::current(),
            Arc::new(crate::runtime::RuntimeChildRunner),
        );
        let reopened = reopened_manager
            .query(
                &task_owner,
                BackgroundTaskQuery {
                    job_id: Some(info.job_id.clone()),
                    include_completed: true,
                    limit: 20,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            reopened[0]
                .output
                .iter()
                .any(|chunk| chunk.text.contains("second"))
        );

        let interactive = manager
            .submit(BackgroundTaskSpec {
                owner: task_owner.clone(),
                command: "read line; printf 'got:%s' \"$line\"".into(),
                cwd: workspace.clone(),
                env: std::collections::BTreeMap::new(),
            })
            .await
            .unwrap();
        manager
            .manage(
                &task_owner,
                &interactive.job_id,
                BackgroundTaskCommand::WriteStdin("hello\n".into()),
            )
            .await
            .unwrap();
        let mut interactive_done = false;
        for _ in 0..20 {
            let current = manager
                .query(
                    &task_owner,
                    BackgroundTaskQuery {
                        job_id: Some(interactive.job_id.clone()),
                        wait_ms: 100,
                        include_completed: true,
                        limit: 20,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            if !current[0].task.status.is_active() {
                assert!(
                    current[0]
                        .output
                        .iter()
                        .any(|chunk| chunk.text.contains("got:hello"))
                );
                interactive_done = true;
                break;
            }
        }
        assert!(interactive_done);

        let long_running = manager
            .submit(BackgroundTaskSpec {
                owner: task_owner.clone(),
                command: "sleep 30".into(),
                cwd: workspace.clone(),
                env: std::collections::BTreeMap::new(),
            })
            .await
            .unwrap();
        let restarted_manager = BackgroundTaskManager::new(
            tokio::runtime::Handle::current(),
            Arc::new(crate::runtime::RuntimeChildRunner),
        );
        let orphan = restarted_manager
            .query(
                &task_owner,
                BackgroundTaskQuery {
                    job_id: Some(long_running.job_id.clone()),
                    include_completed: true,
                    limit: 20,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(orphan[0].task.status, BackgroundTaskStatus::Orphaned);
        assert!(restarted_manager.active_count(&task_owner) == 0);
        manager
            .manage(
                &task_owner,
                &long_running.job_id,
                BackgroundTaskCommand::Terminate,
            )
            .await
            .unwrap();

        // child Agent 也必须走同一套 submit/query/persist 语义；无模型配置让它
        // 在不依赖网络的情况下快速进入失败终态，但仍创建可读取的 child session。
        pipi_venv::agents::create_agent(
            "agent-child",
            "background child",
            Some(workspace.to_str().unwrap()),
            Some(pipi_protocol::permissions::PermissionsConfig {
                tools: vec!["read".into()],
                sandbox: pipi_protocol::permissions::SandboxMode::WorkspaceWrite,
                ..Default::default()
            }),
            None,
            None,
        )
        .unwrap();
        let child_session_events = Arc::new(Mutex::new(Vec::<(String, String, usize)>::new()));
        let session_sink = {
            let child_session_events = child_session_events.clone();
            Arc::new(
                move |agent_name: String, session_id: String, run_id: usize| {
                    child_session_events
                        .lock()
                        .unwrap()
                        .push((agent_name, session_id, run_id));
                },
            )
        };
        let child = manager
            .submit_agent(BackgroundAgentTaskSpec {
                owner: task_owner.clone(),
                target_agent: "agent-child".into(),
                prompt: "不会调用 provider".into(),
                session_sink: Some(session_sink),
            })
            .await
            .unwrap();
        assert_eq!(child.kind, BackgroundTaskKind::Agent);
        assert_eq!(child.target_agent.as_deref(), Some("agent-child"));
        let mut child_done = None;
        for _ in 0..20 {
            let current = manager
                .query(
                    &task_owner,
                    BackgroundTaskQuery {
                        job_id: Some(child.job_id.clone()),
                        wait_ms: 100,
                        include_completed: true,
                        limit: 20,
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
                .pop()
                .unwrap();
            if !current.task.status.is_active() {
                child_done = Some(current);
                break;
            }
        }
        let child_done = child_done.expect("后台 Agent 任务应在测试窗口内结束");
        assert_eq!(child_done.task.kind, BackgroundTaskKind::Agent);
        assert_eq!(child_done.task.status, BackgroundTaskStatus::Failed);
        assert!(child_done.task.child_session_id.is_some());
        assert!(
            child_done
                .task
                .result
                .as_deref()
                .is_some_and(|result| result.contains("no default model configured"))
        );
        let child_session_id = child_done.task.child_session_id.as_deref();
        assert!(child_session_events.lock().unwrap().iter().any(
            |(agent_name, session_id, _run_id)| {
                agent_name == "agent-child" && Some(session_id.as_str()) == child_session_id
            }
        ));
    }
}
