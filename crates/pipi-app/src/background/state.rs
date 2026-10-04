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
