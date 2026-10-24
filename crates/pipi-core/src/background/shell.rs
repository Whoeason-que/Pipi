use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::mpsc;

use pipi_protocol::BackgroundTaskStatus;

use super::persist::{PersistedRecord, append_log, append_record};
use super::state::{JobCommand, JobCore};
use super::{READER_DRAIN_TIMEOUT, TERMINATE_GRACE};

pub(super) struct JobEntry {
    pub(super) core: JobCore,
    pub(super) pid: AtomicU32,
    pub(super) control: mpsc::UnboundedSender<JobCommand>,
    pub(super) log_path: PathBuf,
}

impl JobEntry {
    pub(super) fn finish(
        &self,
        status: BackgroundTaskStatus,
        exit_code: Option<i32>,
        error: Option<String>,
    ) {
        self.pid.store(0, Ordering::Release);
        self.core
            .finish_with(status, error, |info| info.exit_code = exit_code);
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

pub(super) async fn run_job(
    entry: Arc<JobEntry>,
    mut child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    mut control_rx: mpsc::UnboundedReceiver<JobCommand>,
) {
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
                        Some(stdin) => stdin.write_all(&data).await.map_err(|error| format!("Failed to write to background task stdin: {error}")),
                        None => Err("Background task stdin is not available".into()),
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
                Some((stream, text)) => {
                    let _ = append_log(&entry.log_path, &text).await;
                    if let Ok(Some(output)) = entry.core.append_output(stream, text) {
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
                None => break child.wait().await,
            },
        }
    };

    let _ = tokio::time::timeout(READER_DRAIN_TIMEOUT, async {
        let _ = tokio::join!(stdout_task, stderr_task);
    })
    .await;
    while let Ok((stream, text)) = output_rx.try_recv() {
        let _ = append_log(&entry.log_path, &text).await;
        if let Ok(Some(output)) = entry.core.append_output(stream, text) {
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

    let (status, exit_code, error) = match status {
        Ok(exit) if terminated => (BackgroundTaskStatus::Terminated, exit.code(), None),
        Ok(exit) if exit.success() => (BackgroundTaskStatus::Completed, exit.code(), None),
        Ok(exit) => (
            BackgroundTaskStatus::Failed,
            exit.code(),
            Some(format!("Background task exit code {:?}", exit.code())),
        ),
        Err(error) => (
            BackgroundTaskStatus::Failed,
            None,
            Some(format!("Failed to wait for background task exit: {error}")),
        ),
    };
    entry.finish(status, exit_code, error);
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
    entry
        .core
        .finished
        .store(true, std::sync::atomic::Ordering::Release);
}

async fn read_output<R>(
    mut reader: R,
    stream: &'static str,
    sender: mpsc::Sender<(&'static str, String)>,
) where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                if sender
                    .send((
                        stream,
                        String::from_utf8_lossy(&buffer[..size]).into_owned(),
                    ))
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
                .map_err(|error| format!("Cannot terminate background task: {error}"))?;
        }
    }
    #[cfg(not(unix))]
    child
        .start_kill()
        .map_err(|error| format!("Cannot terminate background task: {error}"))?;

    match tokio::time::timeout(TERMINATE_GRACE, child.wait()).await {
        Ok(result) => result
            .map_err(|error| format!("Failed to wait for background task termination: {error}")),
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                let _ = pipi_tools::process::kill_process_group(pid);
            }
            #[cfg(not(unix))]
            child
                .start_kill()
                .map_err(|error| format!("Cannot force terminate background task: {error}"))?;
            child.wait().await.map_err(|error| {
                format!("Failed to wait for background task force termination: {error}")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::MAX_OUTPUT_BYTES;
    use crate::background::test_support::{entry, owner};
    use crate::tools::background::BackgroundTaskOwner;

    #[test]
    fn output_buffer_is_bounded_and_keeps_cursor() {
        let entry = entry(BackgroundTaskStatus::Running);
        for _ in 0..1000 {
            entry
                .core
                .append_output("stdout", "x".repeat(1024))
                .unwrap();
        }
        let snapshot = entry.core.snapshot(0).unwrap();
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
        assert!(entry.core.belongs_to(&owner()));
        assert!(
            !entry
                .core
                .belongs_to(&BackgroundTaskOwner::new("agent", "other", 1))
        );
    }
}
