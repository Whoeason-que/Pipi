use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::{Notify, mpsc};

use crate::tools::background::BackgroundTaskOwner;
use pipi_protocol::{
    BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskOutput, BackgroundTaskSnapshot,
    BackgroundTaskStatus, now_millis,
};

use super::persist::{PersistedRecord, append_log, append_record};
use super::state::{JobCommand, JobState, OutputChunk};
use super::{BackgroundTaskManager, READER_DRAIN_TIMEOUT, TERMINATE_GRACE};

pub(super) struct JobEntry {
    pub(super) owner: Mutex<BackgroundTaskOwner>,
    pub(super) pid: AtomicU32,
    pub(super) state: Mutex<JobState>,
    pub(super) control: mpsc::UnboundedSender<JobCommand>,
    pub(super) notify: Notify,
    pub(super) metadata_path: PathBuf,
    pub(super) log_path: PathBuf,
}

impl JobEntry {
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

    pub(super) fn job_id(&self) -> String {
        self.info().job_id
    }

    pub(super) fn belongs_to(&self, owner: &BackgroundTaskOwner) -> bool {
        // run_id 只记录「由哪一轮提交」，控制权属于 session，而不是某一轮。
        // 下一轮模型必须能查询/终止上一轮留下的后台任务。
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
            .map_err(|error| format!("后台任务状态不可用: {error}"))?;
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
            .map_err(|error| format!("后台任务状态不可用: {error}"))?
            .append_output(stream, text);
        self.notify.notify_waiters();
        Ok(Some(chunk))
    }

    pub(super) fn finish(
        &self,
        status: BackgroundTaskStatus,
        exit_code: Option<i32>,
        error: Option<String>,
    ) {
        self.pid.store(0, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state.info.exit_code = exit_code;
            state.finish(status, error);
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
        {
            let _ = pipi_tools::process::kill_process_group(pid);
        }
    }
}

pub(super) async fn run_job<R1, R2>(
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

async fn terminate_child(child: &mut Child) -> Result<std::process::ExitStatus, String> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // 整组 TERM（进程组在 spawn 时用 process_group(0) 建立）；
            // 目标组已消失（ESRCH）不是错误，helper 内部已按成功处理
            pipi_tools::process::terminate_process_group(pid)
                .map_err(|error| format!("无法终止后台任务: {error}"))?;
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
                let _ = pipi_tools::process::kill_process_group(pid);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::MAX_OUTPUT_BYTES;
    use crate::background::test_support::{entry, owner};

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
}
