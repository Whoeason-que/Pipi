use std::sync::Arc;

use crate::tools::agent::{AgentRunResult, AgentRunStatus};
use crate::tools::background::BackgroundAgentSessionSink;
use pipi_protocol::tools::ToolOutput;
use pipi_protocol::{AbortSignal, BackgroundTaskStatus, ToolResultContent};

use super::ChildAgentRunner;
use super::persist::{PersistedRecord, append_record};
use super::state::JobCore;

pub(super) struct AgentJobEntry {
    pub(super) core: JobCore,
    pub(super) abort: AbortSignal,
}

impl AgentJobEntry {
    /// 终态：子会话 id 与回复正文补进 info，再统一落 status/error。
    pub(super) fn finish(
        &self,
        status: BackgroundTaskStatus,
        result: Option<&AgentRunResult>,
        error: Option<String>,
    ) {
        self.core.finish_with(status, error, |info| {
            if let Some(result) = result {
                info.child_session_id = Some(result.session_id.clone());
                info.result = Some(result.response.clone());
            }
        });
    }
}

impl Drop for AgentJobEntry {
    fn drop(&mut self) {
        if self.core.is_active() {
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
                    if let Ok(Some(output)) = entry.core.append_output("agent", text) {
                        let _ = append_record(
                            &entry.core.metadata_path,
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
        if let Ok(Some(output)) = entry.core.append_output("agent", text) {
            let _ = append_record(
                &entry.core.metadata_path,
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
    if let Ok(snapshot) = entry.core.snapshot(0) {
        let _ = append_record(
            &entry.core.metadata_path,
            &PersistedRecord {
                kind: "finished".into(),
                task: Some(snapshot.task),
                output: None,
            },
        )
        .await;
    }
}
