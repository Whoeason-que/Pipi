//! 后台任务完成的推送式唤醒。
//!
//! 订阅 [`BackgroundTaskManager`] 的终态广播：**订阅开启**（默认）且没有被
//! 查询观测过的 `Completed` / `Failed` 任务，在一个短窗口内合并后写回会话 ——
//! 空闲会话追加 `background_result` 条目并直接起一轮，运行中的会话排进现有
//! 队列（当前轮结束后接着跑，不打断）。
//!
//! 刻意不唤醒的情况：`Terminated`（终止是当前会话自己的决定）、`Orphaned`
//! （重启恢复的旧记录）、订阅关闭（长驻进程）、已被 `query` 读到终态（避免
//! 模型刚等到结果又被推一条）、会话已归档/删除。
//!
//! `RuntimeState::new` 返回 `Self`，拿不到 `Arc`，所以宿主在 `Arc::new(..)`
//! 之后调用一次 [`RuntimeState::start_background_waker`]。

use std::sync::{Arc, Weak};
use std::time::Duration;

use pipi_protocol::session::background_result_tagged_text;
use pipi_protocol::{BackgroundTaskInfo, BackgroundTaskResult, BackgroundTaskStatus};
use tokio::sync::broadcast::error::RecvError;

use super::{RuntimeState, SessionKey};

/// 合并窗口：窗口内完成的任务合成一轮，避免 N 个任务触发 N 轮。
const MERGE_WINDOW: Duration = Duration::from_millis(300);
/// 单条通知保留的输出尾部。
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;
/// 一次唤醒最多合并的任务数（其余的下一轮再送，避免单条消息过长）。
const MAX_RESULTS_PER_WAKE: usize = 8;

/// 一条待投递的通知：投递目标来自任务所有权，不属于通知本身。
struct PendingResult {
    agent_name: String,
    session_id: String,
    result: BackgroundTaskResult,
}

impl RuntimeState {
    /// 启动后台任务唤醒器。宿主在拿到 `Arc<RuntimeState>` 之后调用一次。
    pub fn start_background_waker(self: &Arc<Self>) {
        let mut receiver = self.background.subscribe();
        let weak = Arc::downgrade(self);
        self.spawn_run(async move {
            loop {
                let first = match receiver.recv().await {
                    Ok(info) => info,
                    // 落后（事件被丢弃）不致命：终态各自独立，继续等下一条。
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                let mut batch = Vec::new();
                collect(&weak, first, &mut batch);
                // 固定截止时间：持续有事件也不能无限推迟投递。
                let deadline = tokio::time::Instant::now() + MERGE_WINDOW;
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        event = receiver.recv() => match event {
                            Ok(info) => collect(&weak, info, &mut batch),
                            Err(RecvError::Lagged(_)) => {}
                            Err(RecvError::Closed) => {
                                deliver(&weak, std::mem::take(&mut batch));
                                return;
                            }
                        },
                    }
                }
                deliver(&weak, std::mem::take(&mut batch));
            }
        });
    }
}

/// 哪些终态值得推送：订阅开启、失败或成功、且还没有被一次查询观测过。
/// `Terminated`（终止是当前会话自己的决定）与 `Orphaned`（重启恢复的旧记录）
/// 一律不推送。
fn is_wakeable(info: &BackgroundTaskInfo, observed: bool) -> bool {
    info.subscribe
        && !observed
        && matches!(
            info.status,
            BackgroundTaskStatus::Completed | BackgroundTaskStatus::Failed
        )
}

/// 过滤与排队：订阅、终态、未观测、合并上限。
fn collect(weak: &Weak<RuntimeState>, info: BackgroundTaskInfo, batch: &mut Vec<PendingResult>) {
    if batch.len() >= MAX_RESULTS_PER_WAKE {
        return;
    }
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    // 模型可能正在（或刚刚）用 waitMs 查询并已看到终态：不再重复推。
    if !is_wakeable(&info, runtime.background.was_observed(&info.job_id)) {
        return;
    }
    let result = runtime
        .background
        .completion_result(&info, OUTPUT_TAIL_BYTES);
    batch.push(PendingResult {
        agent_name: info.agent_name,
        session_id: info.session_id,
        result,
    });
}

/// 按会话分组投递；每组只起一轮。
fn deliver(weak: &Weak<RuntimeState>, batch: Vec<PendingResult>) {
    if batch.is_empty() {
        return;
    }
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    // 窗口内可能刚好被一次查询读到终态，投递前再过滤一次。
    let batch: Vec<PendingResult> = batch
        .into_iter()
        .filter(|pending| !runtime.background.was_observed(&pending.result.job_id))
        .collect();
    let mut groups: Vec<((String, String), Vec<BackgroundTaskResult>)> = Vec::new();
    for pending in batch {
        let key = (pending.agent_name, pending.session_id);
        match groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, results)) => results.push(pending.result),
            None => groups.push((key, vec![pending.result])),
        }
    }
    for ((agent_name, session_id), results) in groups {
        if let Err(error) = wake_session(&runtime, &agent_name, &session_id, results) {
            eprintln!(
                "pipi: failed to deliver background results to {agent_name}/{session_id}: {error}"
            );
        }
    }
}

fn wake_session(
    runtime: &Arc<RuntimeState>,
    agent_name: &str,
    session_id: &str,
    results: Vec<BackgroundTaskResult>,
) -> Result<(), String> {
    let text = results
        .iter()
        .map(background_result_tagged_text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let message_id = format!("bg-{}", results[0].job_id);

    // 运行中：排进现有队列，当前轮结束后接着跑（不打断、也不用 waitMs 阻塞）。
    if session_is_running(runtime, agent_name, session_id) {
        if let Err(error) = runtime.queue_prompt(agent_name, session_id, &message_id, &text, None) {
            // 会话可能刚好空闲/关闭：退回新建一轮（下面会再判一次运行态）。
            if !error.contains("not open") {
                return Err(error);
            }
        } else {
            return Ok(());
        }
    }

    match runtime.send_background_results(
        agent_name,
        session_id,
        results.clone(),
        runtime.events().emitter(),
    ) {
        Ok(_) => Ok(()),
        Err(error) if error.contains("is running") => {
            // 检查与发送之间的竞态：改排队。
            runtime.queue_prompt(agent_name, session_id, &message_id, &text, None)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn session_is_running(runtime: &RuntimeState, agent_name: &str, session_id: &str) -> bool {
    runtime
        .sessions
        .lock()
        .ok()
        .and_then(|sessions| {
            sessions
                .get(&SessionKey::new(agent_name, session_id))
                .and_then(|session| session.engine.snapshot().ok())
        })
        .is_some_and(|snapshot| snapshot.running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::BackgroundTaskKind;

    fn info(status: BackgroundTaskStatus, subscribe: bool) -> BackgroundTaskInfo {
        BackgroundTaskInfo {
            job_id: "job-1".into(),
            kind: BackgroundTaskKind::Shell,
            agent_name: "agent".into(),
            session_id: "session".into(),
            run_id: 1,
            command: "cargo test".into(),
            cwd: "/tmp".into(),
            target_agent: None,
            child_session_id: None,
            status,
            started_at: 1,
            completed_at: Some(2),
            exit_code: Some(0),
            output_cursor: 0,
            output_truncated: false,
            subscribe,
            error: None,
            result: None,
        }
    }

    #[test]
    fn only_unobserved_subscribed_success_or_failure_wakes() {
        assert!(is_wakeable(
            &info(BackgroundTaskStatus::Completed, true),
            false
        ));
        assert!(is_wakeable(
            &info(BackgroundTaskStatus::Failed, true),
            false
        ));
        // 订阅关闭（长驻进程）
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Completed, false),
            false
        ));
        // 已经被 waitMs 查询看到
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Completed, true),
            true
        ));
        // 终止与重启孤儿都不唤醒
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Terminated, true),
            false
        ));
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Orphaned, true),
            false
        ));
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Running, true),
            false
        ));
    }
}
