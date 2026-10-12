use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::{Notify, oneshot};

use pipi_protocol::{
    BackgroundTaskInfo, BackgroundTaskOutput, BackgroundTaskSnapshot, BackgroundTaskStatus,
    now_millis,
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

pub(super) struct OutputChunk {
    pub(super) stream: &'static str,
    pub(super) text: String,
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
                .append_output("stdout", String::new())
                .unwrap()
                .is_none()
        );
        assert!(
            agent
                .append_output("agent", String::new())
                .unwrap()
                .is_none()
        );
        for text in ["旧".repeat(MAX_OUTPUT_BYTES / 3), "新尾部".into()] {
            shell.append_output("stdout", text.clone()).unwrap();
            agent.append_output("agent", text).unwrap();
        }
        for snapshot in [shell.snapshot(1).unwrap(), agent.snapshot(1).unwrap()] {
            assert_eq!(snapshot.next_seq, 2);
            assert_eq!(snapshot.task.output_cursor, 2);
            assert!(snapshot.task.output_truncated);
            assert_eq!(snapshot.output.len(), 1);
            assert_eq!(snapshot.output[0].text, "新尾部");
        }
        assert!(shell.snapshot(2).unwrap().output.is_empty());
        assert!(agent.snapshot(2).unwrap().output.is_empty());
        let result = AgentRunResult {
            agent_name: "child".into(),
            session_id: "child-session".into(),
            status: AgentRunStatus::Completed,
            response: "done".into(),
        };
        shell.finish(BackgroundTaskStatus::Completed, Some(0), None);
        agent.finish(BackgroundTaskStatus::Completed, Some(&result), None);
        let shell = shell.snapshot(2).unwrap();
        let agent = agent.snapshot(2).unwrap();
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
            wait_for_state(&shell.state, &shell.notify, 5_000).await;
            wait_for_state(&agent.state, &agent.notify, 5_000).await;
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
            wait_for_state(&still_running.state, &still_running.notify, 5),
        )
        .await
        .unwrap();
    }
}
