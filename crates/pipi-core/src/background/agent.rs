use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::tools::agent::{AgentRunResult, AgentRunStatus};
use crate::tools::background::{BackgroundAgentSessionSink, BackgroundTaskOwner};
use pipi_protocol::tools::ToolOutput;
use pipi_protocol::{
    AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskOutput,
    BackgroundTaskSnapshot, BackgroundTaskStatus, ToolResultContent, now_millis,
};

use super::ChildAgentRunner;
use super::persist::{PersistedRecord, append_record};
use super::state::JobState;

pub(super) struct AgentJobEntry {
    pub(super) owner: Mutex<BackgroundTaskOwner>,
    pub(super) abort: AbortSignal,
    pub(super) state: Mutex<JobState>,
    pub(super) notify: Notify,
    pub(super) metadata_path: PathBuf,
}

impl AgentJobEntry {
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

    pub(super) fn job_id(&self) -> String {
        self.info().job_id
    }

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
            .map_err(|error| format!("后台 Agent 任务状态不可用: {error}"))?;
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
            .map_err(|error| format!("后台 Agent 任务状态不可用: {error}"))?
            .append_output(stream, text);
        self.notify.notify_waiters();
        Ok(Some(chunk))
    }

    pub(super) fn finish(
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
            state.finish(status, error);
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

pub(super) async fn run_agent_job(
    entry: Arc<AgentJobEntry>,
    child_runner: Arc<dyn ChildAgentRunner>,
    target_agent: String,
    prompt: String,
    abort: AbortSignal,
    session_sink: Option<BackgroundAgentSessionSink>,
) {
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<ToolOutput>();
    let child = child_runner.run_child(
        &target_agent,
        &prompt,
        abort,
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
