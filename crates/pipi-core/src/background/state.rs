use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::{Notify, oneshot};

use crate::tools::background::BackgroundTaskOwner;
use pipi_protocol::{
    BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskOutput, BackgroundTaskSnapshot,
    BackgroundTaskStatus, now_millis,
};

use super::MAX_OUTPUT_BYTES;

pub(super) struct JobState {
    pub(super) info: BackgroundTaskInfo,
    pub(super) output: VecDeque<BackgroundTaskOutput>,
    pub(super) output_bytes: usize,
    pub(super) next_seq: u64,
}

impl JobState {
    pub(super) fn snapshot(&self, after_seq: u64) -> BackgroundTaskSnapshot {
        BackgroundTaskSnapshot {
            task: self.info.clone(),
            output: self
                .output
                .iter()
                .filter(|chunk| chunk.seq > after_seq)
                .cloned()
                .collect(),
            next_seq: self.next_seq,
        }
    }

    pub(super) fn append_output(&mut self, stream: &str, text: String) -> BackgroundTaskOutput {
        self.next_seq = self.next_seq.saturating_add(1);
        let chunk = BackgroundTaskOutput {
            seq: self.next_seq,
            stream: stream.to_string(),
            text,
            timestamp: now_millis(),
        };
        self.output_bytes = self.output_bytes.saturating_add(chunk.text.len());
        self.output.push_back(chunk.clone());
        while self.output_bytes > MAX_OUTPUT_BYTES {
            let Some(old) = self.output.pop_front() else {
                break;
            };
            self.output_bytes = self.output_bytes.saturating_sub(old.text.len());
            self.info.output_truncated = true;
        }
        self.info.output_cursor = self.next_seq;
        chunk
    }

    pub(super) fn finish(&mut self, status: BackgroundTaskStatus, error: Option<String>) {
        self.info.status = status;
        self.info.completed_at = Some(now_millis());
        self.info.error = error;
    }
}

pub(super) enum JobCommand {
    Terminate {
        response: oneshot::Sender<Result<(), String>>,
    },
    WriteStdin {
        data: Vec<u8>,
        response: oneshot::Sender<Result<(), String>>,
    },
}

/// 两类后台任务（shell 子进程 / child Agent）共用的记账：身份、状态、等待原语、
/// 元数据路径与有界输出缓冲。
///
/// 差异只有两处在构造时固定：`kind`（写进 `info()` 与元数据）与 `label`
/// （错误文案前缀）。独有字段由各自的 `finish` 在调用 [`Self::finish_with`] 时补写。
pub(super) struct JobCore {
    kind: BackgroundTaskKind,
    pub(super) owner: Mutex<BackgroundTaskOwner>,
    pub(super) state: Mutex<JobState>,
    pub(super) notify: Notify,
    pub(super) metadata_path: PathBuf,
}

impl JobCore {
    /// `info` 是任务起始快照（状态为 Queued/Running）；输出缓冲从空开始。
    pub(super) fn new(
        kind: BackgroundTaskKind,
        owner: BackgroundTaskOwner,
        info: BackgroundTaskInfo,
        metadata_path: PathBuf,
    ) -> Self {
        Self {
            kind,
            owner: Mutex::new(owner),
            state: Mutex::new(JobState {
                info,
                output: VecDeque::new(),
                output_bytes: 0,
                next_seq: 0,
            }),
            notify: Notify::new(),
            metadata_path,
        }
    }

    /// 错误文案前缀（"后台任务" / "后台 Agent 任务"）：两类任务的提示语各自保持原样。
    pub(super) fn label(&self) -> &'static str {
        match self.kind {
            BackgroundTaskKind::Shell => "后台任务",
            BackgroundTaskKind::Agent => "后台 Agent 任务",
        }
    }

    /// 当前 info；状态锁损坏时给一条 Failed 兜底（绝不 panic）。
    pub(super) fn info(&self) -> BackgroundTaskInfo {
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
                kind: self.kind,
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
                error: Some(format!("{}状态锁损坏", self.label())),
                result: None,
            })
    }

    pub(super) fn job_id(&self) -> String {
        self.info().job_id
    }

    /// 控制权属于 session，而不是某一轮：run_id 只记录「由哪一轮提交」，
    /// 下一轮模型必须能查询/终止上一轮留下的后台任务。
    pub(super) fn belongs_to(&self, owner: &BackgroundTaskOwner) -> bool {
        self.owner
            .lock()
            .map(|current| {
                current.agent_name == owner.agent_name && current.session_id == owner.session_id
            })
            .unwrap_or(false)
    }

    pub(super) fn is_active(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.info.status.is_active())
            .unwrap_or(false)
    }

    pub(super) fn snapshot(&self, after_seq: u64) -> Result<BackgroundTaskSnapshot, String> {
        let state = self
            .state
            .lock()
            .map_err(|error| format!("{}状态不可用: {error}", self.label()))?;
        Ok(state.snapshot(after_seq))
    }

    pub(super) fn append_output(
        &self,
        stream: &'static str,
        text: String,
    ) -> Result<Option<BackgroundTaskOutput>, String> {
        if text.is_empty() {
            return Ok(None);
        }
        let chunk = self
            .state
            .lock()
            .map_err(|error| format!("{}状态不可用: {error}", self.label()))?
            .append_output(stream, text);
        self.notify.notify_waiters();
        Ok(Some(chunk))
    }

    /// 终态：`patch` 先补写各自独有字段（shell 的 exit_code / Agent 的 result），
    /// 再统一写 status / completed_at / error，最后唤醒等待者。
    pub(super) fn finish_with(
        &self,
        status: BackgroundTaskStatus,
        error: Option<String>,
        patch: impl FnOnce(&mut BackgroundTaskInfo),
    ) {
        if let Ok(mut state) = self.state.lock() {
            patch(&mut state.info);
            state.finish(status, error);
        }
        self.notify.notify_waiters();
    }

    /// 压缩分叉换 session id：命中本任务则改写 owner 与 info，返回新 info。
    pub(super) fn rekey(
        &self,
        agent_name: &str,
        old_session_id: &str,
        new_session_id: &str,
    ) -> Option<BackgroundTaskInfo> {
        let mut current = self.owner.lock().ok()?;
        if current.agent_name != agent_name || current.session_id != old_session_id {
            return None;
        }
        current.session_id = new_session_id.to_string();
        drop(current);
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        state.info.session_id = new_session_id.to_string();
        drop(state);
        Some(self.info())
    }
}

pub(super) async fn wait_for_state(state: &Mutex<JobState>, notify: &Notify, wait_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        // 先注册等待者再检查终态，避免完成通知落在两者之间而多等整个超时。
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !state
            .lock()
            .map(|state| state.info.status.is_active())
            .unwrap_or(false)
        {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        tokio::select! {
            _ = notified => {},
            _ = tokio::time::sleep(remaining) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::test_support::{agent_entry, entry};
    use crate::tools::agent::{AgentRunResult, AgentRunStatus};

    #[test]
    fn both_job_kinds_preserve_empty_output_cursor_and_incremental_multibyte_tail() {
        let shell = entry(BackgroundTaskStatus::Running);
        let agent = agent_entry(BackgroundTaskStatus::Running);
        assert!(
            shell
                .core
                .append_output("stdout", String::new())
                .unwrap()
                .is_none()
        );
        assert!(
            agent
                .core
                .append_output("agent", String::new())
                .unwrap()
                .is_none()
        );
        for text in ["旧".repeat(MAX_OUTPUT_BYTES / 3), "新尾部".into()] {
            shell.core.append_output("stdout", text.clone()).unwrap();
            agent.core.append_output("agent", text).unwrap();
        }
        for snapshot in [
            shell.core.snapshot(1).unwrap(),
            agent.core.snapshot(1).unwrap(),
        ] {
            assert_eq!(snapshot.next_seq, 2);
            assert_eq!(snapshot.task.output_cursor, 2);
            assert!(snapshot.task.output_truncated);
            assert_eq!(snapshot.output.len(), 1);
            assert_eq!(snapshot.output[0].text, "新尾部");
        }
        assert!(shell.core.snapshot(2).unwrap().output.is_empty());
        assert!(agent.core.snapshot(2).unwrap().output.is_empty());
        let result = AgentRunResult {
            agent_name: "child".into(),
            session_id: "child-session".into(),
            status: AgentRunStatus::Completed,
            response: "done".into(),
        };
        shell.finish(BackgroundTaskStatus::Completed, Some(0), None);
        agent.finish(BackgroundTaskStatus::Completed, Some(&result), None);
        let shell = shell.core.snapshot(2).unwrap();
        let agent = agent.core.snapshot(2).unwrap();
        assert_eq!(shell.task.exit_code, Some(0));
        assert_eq!(agent.task.result.as_deref(), Some("done"));
        assert_eq!(
            agent.task.child_session_id.as_deref(),
            Some("child-session")
        );
        assert!(shell.task.completed_at.is_some() && agent.task.completed_at.is_some());
    }

    #[tokio::test]
    async fn common_wait_returns_on_completion_for_both_kinds_and_bounds_running_wait() {
        let shell = entry(BackgroundTaskStatus::Running);
        let agent = agent_entry(BackgroundTaskStatus::Running);
        let waiter = async {
            wait_for_state(&shell.core.state, &shell.core.notify, 5_000).await;
            wait_for_state(&agent.core.state, &agent.core.notify, 5_000).await;
        };
        let completion = async {
            tokio::task::yield_now().await;
            shell.finish(BackgroundTaskStatus::Completed, Some(0), None);
            agent.finish(BackgroundTaskStatus::Terminated, None, None);
        };
        tokio::time::timeout(Duration::from_millis(500), async {
            tokio::join!(waiter, completion);
        })
        .await
        .unwrap();
        let still_running = entry(BackgroundTaskStatus::Running);
        tokio::time::timeout(
            Duration::from_millis(500),
            wait_for_state(&still_running.core.state, &still_running.core.notify, 5),
        )
        .await
        .unwrap();
    }
}
