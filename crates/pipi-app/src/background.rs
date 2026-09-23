//! 应用层后台任务托管。
//!
//! 这里是唯一持有后台子进程的地方。任务按 Agent/session/run 绑定，输出保留一个
//! 有界内存尾部，同时把完整原始日志写到 Agent 的 jobs/ 目录。元数据使用一行
//! 一个 JSON 记录，应用重启后只把最后仍为 live 的记录展示为 orphaned，绝不
//! 根据旧 PID 自动控制进程。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Notify};

use pipi_core::agents;
use pipi_core::tools::agent::{AgentRunResult, AgentRunStatus};
use pipi_core::tools::background::{
    BackgroundAgentSessionSink, BackgroundAgentTaskSpec, BackgroundTaskCommand,
    BackgroundTaskOwner, BackgroundTaskQuery, BackgroundTaskService, BackgroundTaskSpec,
};
use pipi_core::tools::ToolOutput;
use pipi_core::types::{
    now_millis, AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskOutput,
    BackgroundTaskSnapshot, BackgroundTaskStatus, ToolResultContent,
};

const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const TERMINATE_GRACE: Duration = Duration::from_secs(2);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct BackgroundTaskManager {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: tokio::runtime::Handle,
    jobs: Mutex<HashMap<String, Arc<JobEntry>>>,
    agent_jobs: Mutex<HashMap<String, Arc<AgentJobEntry>>>,
}

struct JobEntry {
    owner: Mutex<BackgroundTaskOwner>,
    pid: AtomicU32,
    state: Mutex<JobState>,
    control: mpsc::UnboundedSender<JobCommand>,
    notify: Notify,
    metadata_path: PathBuf,
    log_path: PathBuf,
}

struct AgentJobEntry {
    owner: Mutex<BackgroundTaskOwner>,
    abort: AbortSignal,
    state: Mutex<JobState>,
    notify: Notify,
    metadata_path: PathBuf,
}

struct JobState {
    info: BackgroundTaskInfo,
    output: VecDeque<BackgroundTaskOutput>,
    output_bytes: usize,
    next_seq: u64,
}

enum JobCommand {
    Terminate {
        response: oneshot::Sender<Result<(), String>>,
    },
    WriteStdin {
        data: Vec<u8>,
        response: oneshot::Sender<Result<(), String>>,
    },
}

struct OutputChunk {
    stream: &'static str,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRecord {
    kind: String,
    #[serde(default)]
    task: Option<BackgroundTaskInfo>,
    #[serde(default)]
    output: Option<BackgroundTaskOutput>,
}

impl BackgroundTaskManager {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            inner: Arc::new(Inner {
                runtime,
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
        if !agents::ensure_real_directory(&agent_dir, "Agent 目录")? {
            return Err(format!("Agent「{agent_name}」不存在"));
        }
        let jobs_dir = agent_dir.join("jobs");
        match agents::ensure_real_directory(&jobs_dir, "后台任务目录")? {
            true => {}
            false => {
                std::fs::create_dir_all(&jobs_dir)
                    .map_err(|error| format!("无法创建后台任务目录: {error}"))?;
                agents::ensure_real_directory(&jobs_dir, "后台任务目录")?;
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
            wait_for_entry(&entry, query.wait_ms).await;
        }
        entry.snapshot(query.after_seq)
    }

    async fn query_live_agent_job(
        &self,
        entry: Arc<AgentJobEntry>,
        query: &BackgroundTaskQuery,
    ) -> Result<BackgroundTaskSnapshot, String> {
        if query.wait_ms > 0 {
            wait_for_agent_entry(&entry, query.wait_ms).await;
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
        let job_id = format!("job-{}", pipi_core::session::new_id());
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
        let job_id = format!("job-{}", pipi_core::session::new_id());
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
                wait_for_agent_entry(&entry, CONTROL_TIMEOUT.as_millis() as u64).await;
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

impl JobEntry {
    fn info(&self) -> BackgroundTaskInfo {
        let owner = self
            .owner
            .lock()
            .map(|owner| owner.clone())
            .unwrap_or_else(|_| BackgroundTaskOwner::new("unknown", "unknown", 0));
        self.state
            .lock()
            .map(|state| state.info.clone())
            .unwrap_or_else(|_| BackgroundTaskInfo {
                job_id: String::new(),
                kind: BackgroundTaskKind::Shell,
                agent_name: owner.agent_name,
                session_id: owner.session_id,
                run_id: owner.run_id,
                command: String::new(),
                cwd: String::new(),
                target_agent: None,
                child_session_id: None,
                status: BackgroundTaskStatus::Failed,
                started_at: 0,
                completed_at: Some(now_millis()),
                exit_code: None,
                output_cursor: 0,
                output_truncated: false,
                error: Some("后台任务状态锁损坏".into()),
                result: None,
            })
    }

    fn job_id(&self) -> String {
        self.info().job_id
    }

    fn belongs_to(&self, owner: &BackgroundTaskOwner) -> bool {
        // run_id 只记录「由哪一轮提交」，控制权属于 session，而不是某一轮。
        // 下一轮模型必须能查询/终止上一轮留下的后台任务。
        self.owner
            .lock()
            .map(|current| {
                current.agent_name == owner.agent_name && current.session_id == owner.session_id
            })
            .unwrap_or(false)
    }

    fn is_active(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.info.status.is_active())
            .unwrap_or(false)
    }

    fn snapshot(&self, after_seq: u64) -> Result<BackgroundTaskSnapshot, String> {
        let state = self
            .state
            .lock()
            .map_err(|error| format!("后台任务状态不可用: {error}"))?;
        Ok(BackgroundTaskSnapshot {
            task: state.info.clone(),
            output: state
                .output
                .iter()
                .filter(|chunk| chunk.seq > after_seq)
                .cloned()
                .collect(),
            next_seq: state.next_seq,
        })
    }

    fn append_output(
        &self,
        stream: &'static str,
        text: String,
    ) -> Result<Option<BackgroundTaskOutput>, String> {
        if text.is_empty() {
            return Ok(None);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|error| format!("后台任务状态不可用: {error}"))?;
        state.next_seq = state.next_seq.saturating_add(1);
        let seq = state.next_seq;
        let chunk = BackgroundTaskOutput {
            seq,
            stream: stream.to_string(),
            text,
            timestamp: now_millis(),
        };
        let persisted_chunk = chunk.clone();
        state.output_bytes = state.output_bytes.saturating_add(chunk.text.len());
        state.output.push_back(chunk);
        while state.output_bytes > MAX_OUTPUT_BYTES {
            let Some(old) = state.output.pop_front() else {
                break;
            };
            state.output_bytes = state.output_bytes.saturating_sub(old.text.len());
            state.info.output_truncated = true;
        }
        state.info.output_cursor = state.next_seq;
        drop(state);
        self.notify.notify_waiters();
        Ok(Some(persisted_chunk))
    }

    fn finish(&self, status: BackgroundTaskStatus, exit_code: Option<i32>, error: Option<String>) {
        self.pid.store(0, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state.info.status = status;
            state.info.exit_code = exit_code;
            state.info.completed_at = Some(now_millis());
            state.info.error = error;
        }
        self.notify.notify_waiters();
    }
}

impl Drop for JobEntry {
    fn drop(&mut self) {
        let pid = self.pid.load(Ordering::Acquire);
        if pid == 0 {
            return;
        }
        // RuntimeState / Tokio runtime shutdown can drop an in-flight driver
        // without giving it another await point. Kill the whole managed group
        // as a last-resort cleanup; normal user termination still gets TERM +
        // grace period + KILL through terminate_child.
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

impl AgentJobEntry {
    fn info(&self) -> BackgroundTaskInfo {
        let owner = self
            .owner
            .lock()
            .map(|owner| owner.clone())
            .unwrap_or_else(|_| BackgroundTaskOwner::new("unknown", "unknown", 0));
        self.state
            .lock()
            .map(|state| state.info.clone())
            .unwrap_or_else(|_| BackgroundTaskInfo {
                job_id: String::new(),
                kind: BackgroundTaskKind::Agent,
                agent_name: owner.agent_name,
                session_id: owner.session_id,
                run_id: owner.run_id,
                command: String::new(),
                cwd: String::new(),
                target_agent: None,
                child_session_id: None,
                status: BackgroundTaskStatus::Failed,
                started_at: 0,
                completed_at: Some(now_millis()),
                exit_code: None,
                output_cursor: 0,
                output_truncated: false,
                error: Some("后台 Agent 任务状态锁损坏".into()),
                result: None,
            })
    }

    fn job_id(&self) -> String {
        self.info().job_id
    }

    fn belongs_to(&self, owner: &BackgroundTaskOwner) -> bool {
        self.owner
            .lock()
            .map(|current| {
                current.agent_name == owner.agent_name && current.session_id == owner.session_id
            })
            .unwrap_or(false)
    }

    fn is_active(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.info.status.is_active())
            .unwrap_or(false)
    }

    fn snapshot(&self, after_seq: u64) -> Result<BackgroundTaskSnapshot, String> {
        let state = self
            .state
            .lock()
            .map_err(|error| format!("后台 Agent 任务状态不可用: {error}"))?;
        Ok(BackgroundTaskSnapshot {
            task: state.info.clone(),
            output: state
                .output
                .iter()
                .filter(|chunk| chunk.seq > after_seq)
                .cloned()
                .collect(),
            next_seq: state.next_seq,
        })
    }

    fn append_output(
        &self,
        stream: &'static str,
        text: String,
    ) -> Result<Option<BackgroundTaskOutput>, String> {
        if text.is_empty() {
            return Ok(None);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|error| format!("后台 Agent 任务状态不可用: {error}"))?;
        state.next_seq = state.next_seq.saturating_add(1);
        let chunk = BackgroundTaskOutput {
            seq: state.next_seq,
            stream: stream.to_string(),
            text,
            timestamp: now_millis(),
        };
        let persisted = chunk.clone();
        state.output_bytes = state.output_bytes.saturating_add(chunk.text.len());
        state.output.push_back(chunk);
        while state.output_bytes > MAX_OUTPUT_BYTES {
            let Some(old) = state.output.pop_front() else {
                break;
            };
            state.output_bytes = state.output_bytes.saturating_sub(old.text.len());
            state.info.output_truncated = true;
        }
        state.info.output_cursor = state.next_seq;
        drop(state);
        self.notify.notify_waiters();
        Ok(Some(persisted))
    }

    fn finish(
        &self,
        status: BackgroundTaskStatus,
        result: Option<&AgentRunResult>,
        error: Option<String>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(result) = result {
                state.info.child_session_id = Some(result.session_id.clone());
                state.info.result = Some(result.response.clone());
            }
            state.info.status = status;
            state.info.completed_at = Some(now_millis());
            state.info.error = error;
        }
        self.notify.notify_waiters();
    }
}

impl Drop for AgentJobEntry {
    fn drop(&mut self) {
        if self.is_active() {
            self.abort.abort();
        }
    }
}

fn tool_output_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text { text } => Some(text.as_str()),
            ToolResultContent::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn run_agent_job(
    entry: Arc<AgentJobEntry>,
    target_agent: String,
    prompt: String,
    abort: AbortSignal,
    session_sink: Option<BackgroundAgentSessionSink>,
) {
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<ToolOutput>();
    let child = crate::runtime::run_agent_once_inner_with_sink(
        &target_agent,
        &prompt,
        abort,
        Duration::from_secs(crate::runtime::CHILD_RUN_TIMEOUT_SECS),
        Some(progress_tx),
        session_sink,
    );
    tokio::pin!(child);
    let outcome = loop {
        tokio::select! {
            result = &mut child => break result,
            update = progress_rx.recv() => match update {
                Some(update) => {
                    let text = tool_output_text(&update);
                    if let Ok(Some(output)) = entry.append_output("agent", text) {
                        let _ = append_record(
                            &entry.metadata_path,
                            &PersistedRecord {
                                kind: "output".into(),
                                task: None,
                                output: Some(output),
                            },
                        )
                        .await;
                    }
                }
                None => break child.await,
            },
        }
    };
    while let Ok(update) = progress_rx.try_recv() {
        let text = tool_output_text(&update);
        if let Ok(Some(output)) = entry.append_output("agent", text) {
            let _ = append_record(
                &entry.metadata_path,
                &PersistedRecord {
                    kind: "output".into(),
                    task: None,
                    output: Some(output),
                },
            )
            .await;
        }
    }

    let (status, result, error) = match outcome {
        Ok(result) => {
            let status = match result.status {
                AgentRunStatus::Completed => BackgroundTaskStatus::Completed,
                AgentRunStatus::Aborted => BackgroundTaskStatus::Terminated,
                AgentRunStatus::Failed | AgentRunStatus::Pending => BackgroundTaskStatus::Failed,
            };
            (status, Some(result), None)
        }
        Err(error) => (BackgroundTaskStatus::Failed, None, Some(error)),
    };
    entry.finish(status, result.as_ref(), error);
    if let Ok(snapshot) = entry.snapshot(0) {
        let _ = append_record(
            &entry.metadata_path,
            &PersistedRecord {
                kind: "finished".into(),
                task: Some(snapshot.task),
                output: None,
            },
        )
        .await;
    }
}

async fn run_job<R1, R2>(
    _manager: BackgroundTaskManager,
    entry: Arc<JobEntry>,
    mut child: Child,
    stdout: R1,
    stderr: R2,
    mut control_rx: mpsc::UnboundedReceiver<JobCommand>,
) where
    R1: AsyncRead + Unpin + Send + 'static,
    R2: AsyncRead + Unpin + Send + 'static,
{
    // 有界输出队列：日志写盘变慢时让读取任务反压到子进程 pipe，不能让一个
    // 失控的后台进程把 Pipi 内存吃光。
    let (output_tx, mut output_rx) = mpsc::channel(128);
    let stdout_task = tokio::spawn(read_output(stdout, "stdout", output_tx.clone()));
    let stderr_task = tokio::spawn(read_output(stderr, "stderr", output_tx));
    let mut stdin = child.stdin.take();
    let mut terminated = false;
    let status = loop {
        tokio::select! {
            result = child.wait() => break result,
            message = control_rx.recv() => match message {
                Some(JobCommand::WriteStdin { data, response }) => {
                    let result = match stdin.as_mut() {
                        Some(stdin) => stdin.write_all(&data).await.map_err(|error| format!("写入后台任务 stdin 失败: {error}")),
                        None => Err("后台任务 stdin 不可用".into()),
                    };
                    let _ = response.send(result);
                }
                Some(JobCommand::Terminate { response }) => {
                    terminated = true;
                    match terminate_child(&mut child).await {
                        Ok(exit) => {
                            let _ = response.send(Ok(()));
                            break Ok(exit);
                        }
                        Err(error) => {
                            let _ = response.send(Err(error.clone()));
                            break Err(std::io::Error::other(error));
                        }
                    }
                }
                None => break child.wait().await,
            },
            chunk = output_rx.recv() => match chunk {
                Some(chunk) => {
                    let _ = append_log(&entry.log_path, &chunk.text).await;
                    if let Ok(Some(output)) = entry.append_output(chunk.stream, chunk.text) {
                        let _ = append_record(
                            &entry.metadata_path,
                            &PersistedRecord {
                                kind: "output".into(),
                                task: None,
                                output: Some(output),
                            },
                        )
                        .await;
                    }
                }
                None => break child.wait().await,
            },
        }
    };

    let _ = tokio::time::timeout(READER_DRAIN_TIMEOUT, async {
        let _ = tokio::join!(stdout_task, stderr_task);
    })
    .await;
    while let Ok(chunk) = output_rx.try_recv() {
        let _ = append_log(&entry.log_path, &chunk.text).await;
        if let Ok(Some(output)) = entry.append_output(chunk.stream, chunk.text) {
            let _ = append_record(
                &entry.metadata_path,
                &PersistedRecord {
                    kind: "output".into(),
                    task: None,
                    output: Some(output),
                },
            )
            .await;
        }
    }

    let (status, exit_code, error) = match status {
        Ok(exit) if terminated => (BackgroundTaskStatus::Terminated, exit.code(), None),
        Ok(exit) if exit.success() => (BackgroundTaskStatus::Completed, exit.code(), None),
        Ok(exit) => (
            BackgroundTaskStatus::Failed,
            exit.code(),
            Some(format!("后台任务退出码 {:?}", exit.code())),
        ),
        Err(error) => (
            BackgroundTaskStatus::Failed,
            None,
            Some(format!("等待后台任务结束失败: {error}")),
        ),
    };
    entry.finish(status, exit_code, error);
    if let Ok(snapshot) = entry.snapshot(0) {
        let _ = append_record(
            &entry.metadata_path,
            &PersistedRecord {
                kind: "finished".into(),
                task: Some(snapshot.task),
                output: None,
            },
        )
        .await;
    }
}

async fn read_output<R>(mut reader: R, stream: &'static str, sender: mpsc::Sender<OutputChunk>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                if sender
                    .send(OutputChunk {
                        stream,
                        text: String::from_utf8_lossy(&buffer[..size]).into_owned(),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

async fn wait_for_entry(entry: &JobEntry, wait_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        if !entry.is_active() {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        tokio::select! {
            _ = entry.notify.notified() => {}
            _ = tokio::time::sleep(remaining) => return,
        }
    }
}

async fn wait_for_agent_entry(entry: &AgentJobEntry, wait_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        if !entry.is_active() {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        tokio::select! {
            _ = entry.notify.notified() => {}
            _ = tokio::time::sleep(remaining) => return,
        }
    }
}

async fn terminate_child(child: &mut Child) -> Result<std::process::ExitStatus, String> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let result = unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(format!("无法终止后台任务进程组: {error}"));
                }
            }
        }
    }
    #[cfg(not(unix))]
    child
        .start_kill()
        .map_err(|error| format!("无法终止后台任务: {error}"))?;

    match tokio::time::timeout(TERMINATE_GRACE, child.wait()).await {
        Ok(result) => result.map_err(|error| format!("等待后台任务终止失败: {error}")),
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            }
            #[cfg(not(unix))]
            child
                .start_kill()
                .map_err(|error| format!("无法强制终止后台任务: {error}"))?;
            child
                .wait()
                .await
                .map_err(|error| format!("等待后台任务强制终止失败: {error}"))
        }
    }
}

async fn append_log(path: &Path, text: &str) -> Result<(), String> {
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

async fn append_record(path: &Path, record: &PersistedRecord) -> Result<(), String> {
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

async fn load_persisted_snapshot(
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

fn validate_job_id(job_id: &str) -> Result<(), String> {
    if job_id.is_empty()
        || !job_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err("非法后台任务 ID".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> BackgroundTaskOwner {
        BackgroundTaskOwner::new("agent", "session", 1)
    }

    fn entry(status: BackgroundTaskStatus) -> JobEntry {
        let (control, _rx) = mpsc::unbounded_channel();
        JobEntry {
            owner: Mutex::new(owner()),
            pid: AtomicU32::new(0),
            state: Mutex::new(JobState {
                info: BackgroundTaskInfo {
                    job_id: "job-1".into(),
                    kind: BackgroundTaskKind::Shell,
                    agent_name: "agent".into(),
                    session_id: "session".into(),
                    run_id: 1,
                    command: "echo".into(),
                    cwd: "/tmp".into(),
                    target_agent: None,
                    child_session_id: None,
                    status,
                    started_at: 1,
                    completed_at: None,
                    exit_code: None,
                    output_cursor: 0,
                    output_truncated: false,
                    error: None,
                    result: None,
                },
                output: VecDeque::new(),
                output_bytes: 0,
                next_seq: 0,
            }),
            control,
            notify: Notify::new(),
            metadata_path: PathBuf::new(),
            log_path: PathBuf::new(),
        }
    }

    #[test]
    fn output_buffer_is_bounded_and_keeps_cursor() {
        let entry = entry(BackgroundTaskStatus::Running);
        for _ in 0..1000 {
            entry.append_output("stdout", "x".repeat(1024)).unwrap();
        }
        let snapshot = entry.snapshot(0).unwrap();
        assert_eq!(snapshot.next_seq, 1000);
        assert!(snapshot.task.output_truncated);
        assert!(
            snapshot
                .output
                .iter()
                .map(|item| item.text.len())
                .sum::<usize>()
                <= MAX_OUTPUT_BYTES
        );
    }

    #[test]
    fn owner_check_is_strict() {
        let entry = entry(BackgroundTaskStatus::Completed);
        assert!(entry.belongs_to(&owner()));
        assert!(!entry.belongs_to(&BackgroundTaskOwner::new("agent", "other", 1)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn submit_query_manage_and_restart_are_session_scoped() {
        let _home_guard = crate::HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pipi-background-{}-{}",
            std::process::id(),
            pipi_core::session::new_id()
        ));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::env::set_var("HOME", &home);
        let permissions = pipi_core::permissions::PermissionsConfig {
            tools: pipi_core::permissions::BACKGROUND_TASK_TOOLS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            sandbox: pipi_core::permissions::SandboxMode::DangerFullAccess,
            ..Default::default()
        };
        pipi_core::agents::create_agent(
            "background-test",
            "background test",
            Some(workspace.to_str().unwrap()),
            Some(permissions),
            None,
            None,
        )
        .unwrap();

        let manager = BackgroundTaskManager::new(tokio::runtime::Handle::current());
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
        assert!(snapshot
            .output
            .iter()
            .any(|chunk| chunk.text.contains("first")));
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
        let reopened_manager = BackgroundTaskManager::new(tokio::runtime::Handle::current());
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
        assert!(reopened[0]
            .output
            .iter()
            .any(|chunk| chunk.text.contains("second")));

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
                assert!(current[0]
                    .output
                    .iter()
                    .any(|chunk| chunk.text.contains("got:hello")));
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
        let restarted_manager = BackgroundTaskManager::new(tokio::runtime::Handle::current());
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
        pipi_core::agents::create_agent(
            "agent-child",
            "background child",
            Some(workspace.to_str().unwrap()),
            Some(pipi_core::permissions::PermissionsConfig {
                tools: vec!["read".into()],
                sandbox: pipi_core::permissions::SandboxMode::WorkspaceWrite,
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
        assert!(child_done
            .task
            .result
            .as_deref()
            .is_some_and(|result| result.contains("还未配置默认模型")));
        let child_session_id = child_done.task.child_session_id.as_deref();
        assert!(child_session_events.lock().unwrap().iter().any(
            |(agent_name, session_id, _run_id)| {
                agent_name == "agent-child" && Some(session_id.as_str()) == child_session_id
            }
        ));
    }
}
